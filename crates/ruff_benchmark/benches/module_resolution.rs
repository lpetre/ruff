//! Micro-benchmark for the module resolver's search-path scaling.
//!
//! Each benchmark configures a fresh `ProjectDatabase` with `n` extra
//! search paths (each containing a single Python module) and then resolves
//! a fixed batch of module names. The batch contains a mix of names that
//! exist in one specific extra path, names that exist nowhere, and stdlib
//! names. The interesting variable is the number of search paths — for the
//! "monorepo" case (`n=600`) the resolver has to consider many candidate
//! locations per name on the unoptimized path.

#![allow(clippy::disallowed_names)]

use ruff_benchmark::criterion;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
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

/// Names we attempt to resolve per benchmark iteration.
///
/// Five `target_*` names are seeded into the first five extra paths (one
/// per path), so for `n >= 5` they all resolve to a specific extra path.
/// `nonexistent_*` names never resolve. The stdlib names exercise the
/// non-shadowable / stdlib-only code path.
const TARGET_NAMES: &[&str] = &[
    "target_0",
    "target_1",
    "target_2",
    "target_3",
    "target_4",
    "nonexistent_0",
    "nonexistent_1",
    "nonexistent_2",
    "nonexistent_3",
    "nonexistent_4",
    "nonexistent_5",
    "nonexistent_6",
    "nonexistent_7",
    "os",
    "sys",
    "typing",
    "collections",
    "itertools",
    "functools",
];

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

        // Spread the target_* modules across the first few extra paths
        // so the resolver has a real candidate to narrow to.
        if i < TARGET_NAMES.len() {
            let target = SystemPathBuf::from(format!("{dir}/{}.py", TARGET_NAMES[i]));
            fs.write_file_all(&target, "x = 0").unwrap();
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

    let resolves = TARGET_NAMES
        .iter()
        .map(|n| ModuleName::new_static(n).unwrap())
        .collect();

    Case {
        db,
        importing_file,
        resolves,
    }
}

fn benchmark_resolve(criterion: &mut Criterion, n: usize) {
    setup_rayon();

    criterion.bench_function(&format!("ty_module_resolver[n_extra_paths={n}]"), |b| {
        b.iter_batched(
            || setup_case(n),
            |case| {
                let Case {
                    db,
                    importing_file,
                    resolves,
                } = case;
                for name in &resolves {
                    let _ = resolve_module(&db, importing_file, name);
                }
            },
            BatchSize::PerIteration,
        );
    });
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

fn resolve_0_paths(criterion: &mut Criterion) {
    benchmark_resolve(criterion, PATH_COUNTS[0]);
}
fn resolve_1_path(criterion: &mut Criterion) {
    benchmark_resolve(criterion, PATH_COUNTS[1]);
}
fn resolve_5_paths(criterion: &mut Criterion) {
    benchmark_resolve(criterion, PATH_COUNTS[2]);
}
fn resolve_25_paths(criterion: &mut Criterion) {
    benchmark_resolve(criterion, PATH_COUNTS[3]);
}
fn resolve_125_paths(criterion: &mut Criterion) {
    benchmark_resolve(criterion, PATH_COUNTS[4]);
}
fn resolve_600_paths(criterion: &mut Criterion) {
    benchmark_resolve(criterion, PATH_COUNTS[5]);
}

criterion_group!(
    module_resolution_search_path_scaling,
    resolve_0_paths,
    resolve_1_path,
    resolve_5_paths,
    resolve_25_paths,
    resolve_125_paths,
    resolve_600_paths,
);
criterion_main!(module_resolution_search_path_scaling);
