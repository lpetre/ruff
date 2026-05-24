//! Micro-benchmark for the module resolver's search-path scaling.
//!
//! Each benchmark configures a fresh `ProjectDatabase` with `n` extra
//! search paths (each containing a single Python module) and then resolves
//! a fixed batch of module names. The batch mixes names that exist in one
//! specific extra path, names that exist nowhere, and stdlib names. The
//! interesting variable is the number of search paths — for the "monorepo"
//! case (`n=600`) the resolver has to consider many candidate locations
//! per name on the unoptimized path.

#![allow(clippy::disallowed_names)]

use ruff_benchmark::criterion;

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use rayon::ThreadPoolBuilder;

use ruff_db::files::{File, system_path_to_file};
use ruff_db::system::{MemoryFileSystem, SystemPath, SystemPathBuf, TestSystem};
use ty_module_resolver::{ModuleName, resolve_module};
use ty_project::metadata::options::{EnvironmentOptions, Options};
use ty_project::metadata::python_version::SupportedPythonVersion;
use ty_project::metadata::value::{RangedValue, RelativePathBuf};
use ty_project::{ProjectDatabase, ProjectMetadata};

/// Sizes representative of: no extras, single extra, small/medium/large
/// monorepos, and an X-Large monorepo.
const PATH_COUNTS: &[usize] = &[0, 1, 5, 25, 125, 600];

/// Names seeded into the first `SEEDED_TARGETS.len()` extra paths (one
/// per path). For `n >= SEEDED_TARGETS.len()` they all resolve to a
/// specific extra path; for smaller `n` only a prefix of them resolves.
const SEEDED_TARGETS: &[&str] = &["target_0", "target_1", "target_2", "target_3", "target_4"];

/// Names that never exist in any search path — exercises the "no
/// candidate" branch.
const NONEXISTENT_NAMES: &[&str] = &[
    "nonexistent_0",
    "nonexistent_1",
    "nonexistent_2",
    "nonexistent_3",
    "nonexistent_4",
    "nonexistent_5",
    "nonexistent_6",
    "nonexistent_7",
];

/// Stdlib names — resolve into typeshed. Includes the non-shadowable
/// `sys` so we cover that filter too.
const STDLIB_NAMES: &[&str] = &["os", "sys", "typing", "collections", "itertools", "functools"];

struct Case {
    db: ProjectDatabase,
    importing_file: File,
    resolves: Vec<ModuleName>,
}

fn setup_case(n: usize) -> Case {
    let system = TestSystem::default();
    let fs: MemoryFileSystem = system.memory_file_system().clone();

    let mut extra_paths = Vec::with_capacity(n);
    for i in 0..n {
        let dir = format!("/extra/p{i}");
        let filler = SystemPathBuf::from(format!("{dir}/mod{i}.py"));
        fs.write_file_all(&filler, "x = 0").unwrap();
        extra_paths.push(RelativePathBuf::cli(SystemPath::new(&dir)));

        // Seed exactly one `target_*` module into each of the first
        // SEEDED_TARGETS.len() extra paths so the resolver has a real
        // candidate to narrow to. `NONEXISTENT_NAMES` and `STDLIB_NAMES`
        // must NOT be seeded here or they'd lose their negative-lookup
        // and stdlib-only semantics.
        if let Some(target) = SEEDED_TARGETS.get(i) {
            let target_path = SystemPathBuf::from(format!("{dir}/{target}.py"));
            fs.write_file_all(&target_path, "x = 0").unwrap();
        }
    }

    let importing_path = SystemPathBuf::from("/src/test.py");
    fs.write_file_all(&importing_path, "").unwrap();

    let src_root = SystemPath::new("/src");
    let mut metadata = ProjectMetadata::discover(src_root, &system).unwrap();
    metadata.apply_options(Options {
        environment: Some(EnvironmentOptions {
            python_version: Some(RangedValue::cli(SupportedPythonVersion::Py312)),
            extra_paths: Some(extra_paths),
            ..EnvironmentOptions::default()
        }),
        ..Options::default()
    });

    let db = ProjectDatabase::fallible(metadata, system).unwrap();
    let importing_file = system_path_to_file(&db, &importing_path).unwrap();

    let resolves = SEEDED_TARGETS
        .iter()
        .chain(NONEXISTENT_NAMES.iter())
        .chain(STDLIB_NAMES.iter())
        .map(|name| ModuleName::new_static(name).unwrap())
        .collect();

    Case {
        db,
        importing_file,
        resolves,
    }
}

fn benchmark_search_path_scaling(criterion: &mut Criterion) {
    setup_rayon();

    let mut group = criterion.benchmark_group("ty_module_resolver");
    for &n in PATH_COUNTS {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || setup_case(n),
                |case| {
                    let Case {
                        db,
                        importing_file,
                        resolves,
                    } = case;
                    for name in &resolves {
                        black_box(resolve_module(&db, importing_file, name));
                    }
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

static RAYON_INITIALIZED: std::sync::Once = std::sync::Once::new();

fn setup_rayon() {
    RAYON_INITIALIZED.call_once(|| {
        ThreadPoolBuilder::new()
            .num_threads(1)
            .use_current_thread()
            .build_global()
            .unwrap();
    });
}

criterion_group!(module_resolution, benchmark_search_path_scaling);
criterion_main!(module_resolution);
