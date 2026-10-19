use std::collections::BTreeMap;

use aver::env::{self, EnvError, StaticResolver};
use aver::store::Store;
use aver_core::*;
use tempfile::TempDir;

fn p<T: std::str::FromStr>(s: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    s.parse().unwrap()
}

fn open() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("aver.db")).unwrap();
    (dir, store)
}

/// Stores a tree holding `files` (path, contents) and returns its root.
fn tree(store: &Store, files: &[(&str, &str)]) -> TreeId {
    fn build(store: &Store, files: &[(Vec<&str>, &str)]) -> TreeId {
        let mut groups: BTreeMap<&str, Vec<(Vec<&str>, &str)>> = BTreeMap::new();
        let mut entries = Vec::new();
        for (segments, content) in files {
            if segments.len() == 1 {
                let blob = store.put_blob(content.as_bytes()).unwrap();
                entries.push(TreeEntry {
                    name: p(segments[0]),
                    node: Node::File(blob),
                });
            } else {
                groups
                    .entry(segments[0])
                    .or_default()
                    .push((segments[1..].to_vec(), content));
            }
        }
        for (name, children) in groups {
            entries.push(TreeEntry {
                name: p(name),
                node: Node::Tree(build(store, &children)),
            });
        }
        store.put(&Tree::new(Set::new(entries).unwrap())).unwrap()
    }
    let split: Vec<_> = files
        .iter()
        .map(|(path, content)| (path.split('/').collect(), *content))
        .collect();
    build(store, &split)
}

fn digest(byte: &str) -> Digest {
    p(&format!("sha256:{}", byte.repeat(32)))
}

/// The environment `aver ci` pins, with checks detected where `aver.toml`
/// names none.
fn resolve(files: &[(&str, &str)]) -> Result<Environment, EnvError> {
    resolve_with(files, env::Detection::Presets)
}

fn resolve_with(
    files: &[(&str, &str)],
    detection: env::Detection,
) -> Result<Environment, EnvError> {
    let (_dir, store) = open();
    let root = tree(&store, files);
    let resolver = StaticResolver::default().with("node:22", digest("cd"));
    env::environment(&store, &root, &resolver, detection)
}

fn invalid(files: &[(&str, &str)]) -> String {
    match resolve(files) {
        Err(e @ (EnvError::Invalid(_) | EnvError::Syntax(_))) => e.to_string(),
        other => panic!("expected an invalid config, got {other:?}"),
    }
}

fn config(toml: &str) -> String {
    invalid(&[("aver.toml", toml), ("package.json", "{}")])
}

#[test]
fn a_tree_without_config_gets_the_pinned_defaults() {
    let env = resolve(&[("package.json", "{}"), ("src/index.js", "")]).unwrap();
    assert_eq!(env.platform, env::host_platform());
    assert_eq!(
        env.platform,
        if cfg!(target_arch = "x86_64") {
            Platform::LinuxAmd64
        } else {
            Platform::LinuxArm64
        }
    );
    assert_eq!(env.image.reference, "node:22-bookworm-slim");
    assert_eq!(
        env.image.digest.as_str(),
        "sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392"
    );
    assert_eq!(env.lockfile, None);
    assert_eq!(
        env.install,
        Install {
            argv: ["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"]
                .map(String::from)
                .to_vec(),
            allow_scripts: false,
            rebuild: true,
        }
    );
    assert_eq!(
        env.schema,
        EnvSchema::V4,
        "a detected check is unapproved, a v4 field"
    );
    let checks: Vec<_> = env.checks.iter().cloned().collect();
    assert_eq!(
        checks,
        vec![CheckDef {
            id: p("test"),
            argv: vec!["npm".into(), "test".into()],
            cwd: None,
            timeout_ms: None,
            report: None,
            setup: Vec::new(),
            image: None,
            deps: None,
            covers: aver_core::Set::empty(),
            min_tests: None,
            approved: false,
        }]
    );
    assert_eq!(
        env.limits,
        Limits {
            cpu_millis: 2000,
            memory_mb: 2048,
            pids: 512,
            timeout_ms: 300_000,
            output_bytes: 65_536,
        }
    );
    assert!(env.vars.is_empty());

    let with_lock = resolve(&[("package-lock.json", "{\"lockfileVersion\":3}")]).unwrap();
    assert_eq!(
        with_lock.lockfile,
        Some(Lockfile {
            path: p("package-lock.json"),
            blob: BlobId::of_bytes(b"{\"lockfileVersion\":3}"),
        })
    );
    // A lockfile below the root is not picked up without configuration.
    assert_eq!(
        resolve(&[("app/package-lock.json", "{}")])
            .unwrap()
            .lockfile,
        None
    );
}

#[test]
fn a_full_config_is_parsed_exactly() {
    let toml = r#"
[env]
image = "node:22-bookworm-slim@sha256:1111111111111111111111111111111111111111111111111111111111111111"
lockfile = "web/app/package-lock.json"
install = ["npm", "ci"]
allow_scripts = true

[limits]
cpus = 1.5
memory_mb = 1024
pids = 128
timeout_s = 60
output_bytes = 4096

[[check]]
id = "unit"
run = ["node", "--test", "--test-reporter=junit", "--test-reporter-destination=/aver/out/unit.xml"]
report = { format = "junit", path = "/aver/out/unit.xml" }

[[check]]
id = "lint"
run = ["npx", "--no-install", "eslint", "."]
cwd = "web/app"
timeout_s = 30

[vars]
NODE_OPTIONS = "--max-old-space-size=512"
TZ = "UTC"

[[unit]]
id = "billing"
paths = ["src/billing/**"]
"#;
    let env = resolve(&[
        ("aver.toml", toml),
        ("web/app/package-lock.json", "{\"lock\":1}"),
    ])
    .unwrap();
    let expected = Environment {
        schema: EnvSchema::V0,
        tests: Set::empty(),
        platform: env::host_platform(),
        image: Image {
            reference: "node:22-bookworm-slim".into(),
            digest: digest("11"),
        },
        lockfile: Some(Lockfile {
            path: p("web/app/package-lock.json"),
            blob: BlobId::of_bytes(b"{\"lock\":1}"),
        }),
        install: Install {
            argv: vec!["npm".into(), "ci".into()],
            allow_scripts: true,
            rebuild: false,
        },
        checks: Set::new(vec![
            CheckDef {
                id: p("unit"),
                argv: [
                    "node",
                    "--test",
                    "--test-reporter=junit",
                    "--test-reporter-destination=/aver/out/unit.xml",
                ]
                .map(String::from)
                .to_vec(),
                cwd: None,
                timeout_ms: None,
                report: Some(Report {
                    format: ReportFormat::Junit,
                    path: p("/aver/out/unit.xml"),
                }),
                setup: Vec::new(),
                image: None,
                deps: None,
                covers: aver_core::Set::empty(),
                min_tests: None,
                approved: true,
            },
            CheckDef {
                id: p("lint"),
                argv: ["npx", "--no-install", "eslint", "."]
                    .map(String::from)
                    .to_vec(),
                cwd: Some(p("web/app")),
                timeout_ms: Some(30_000),
                report: None,
                setup: Vec::new(),
                image: None,
                deps: None,
                covers: aver_core::Set::empty(),
                min_tests: None,
                approved: true,
            },
        ])
        .unwrap(),
        limits: Limits {
            cpu_millis: 1500,
            memory_mb: 1024,
            pids: 128,
            timeout_ms: 60_000,
            output_bytes: 4096,
        },
        vars: [("NODE_OPTIONS", "--max-old-space-size=512"), ("TZ", "UTC")]
            .into_iter()
            .map(|(k, v)| (p(k), v.to_owned()))
            .collect(),
        layers: aver_core::Set::empty(),
    };
    assert_eq!(env, expected);
    assert_eq!(
        env.checks.as_slice()[0].id.as_str(),
        "lint",
        "checks are sorted"
    );
}

#[test]
fn partial_config_falls_back_to_defaults_per_field() {
    let env = resolve(&[
        ("aver.toml", "[limits]\nmemory_mb = 512\n"),
        ("package-lock.json", "{}"),
    ])
    .unwrap();
    assert_eq!(env.limits.memory_mb, 512);
    assert_eq!(env.limits.cpu_millis, 2000);
    assert_eq!(env.checks.len(), 1);
    assert_eq!(env.lockfile.unwrap().path.as_str(), "package-lock.json");

    let no_checks = resolve(&[("aver.toml", "check = []\n")]).unwrap();
    assert!(no_checks.checks.is_empty());
}

#[test]
fn tag_only_images_are_pinned_through_the_resolver() {
    let env = resolve(&[("aver.toml", "[env]\nimage = \"node:22\"\n")]).unwrap();
    assert_eq!(env.image.reference, "node:22");
    assert_eq!(env.image.digest, digest("cd"));

    let err = resolve(&[("aver.toml", "[env]\nimage = \"python:3\"\n")]).unwrap_err();
    assert!(
        matches!(&err, EnvError::Resolve { reference, .. } if reference == "python:3"),
        "{err}"
    );
}

#[test]
fn invalid_configs_are_rejected_with_the_offending_field() {
    let cases: &[(&str, &str)] = &[
        ("[vars]\nGITHUB_TOKEN = \"x\"\n", "secret"),
        ("[vars]\napi_key = \"x\"\n", "secret"),
        ("[vars]\n\"1BAD\" = \"x\"\n", "vars.1BAD"),
        ("[[check]]\nid = \"Unit\"\nrun = [\"x\"]\n", "check.id"),
        ("[[check]]\nid = \"u\"\nrun = []\n", "must not be empty"),
        (
            "[[check]]\nid = \"u\"\nrun = [\"x\"]\n[[check]]\nid = \"u\"\nrun = [\"y\"]\n",
            "declared twice",
        ),
        (
            "[[check]]\nid = \"u\"\nrun = [\"x\"]\nreport = { format = \"junit\", path = \"/tmp/u.xml\" }\n",
            "report.path",
        ),
        (
            "[[check]]\nid = \"u\"\nrun = [\"x\"]\nreport = { format = \"junit\", path = \"/aver/out/../etc\" }\n",
            "report.path",
        ),
        (
            "[[check]]\nid = \"u\"\nrun = [\"x\"]\nreport = { format = \"xunit\", path = \"/aver/out/u\" }\n",
            "report.format",
        ),
        ("[[check]]\nid = \"u\"\nrun = [\"x\"]\ncwd = \"../up\"\n", "cwd"),
        ("[env]\nlockfile = \"missing.json\"\n", "env.lockfile"),
        ("[env]\nimage = \"node@sha256:abc\"\n", "env.image"),
        ("[env]\nimage = \"--privileged\"\n", "env.image"),
        ("[env]\nimage = \"-v@sha256:{}\"\n", "env.image"),
        ("[env]\nimage = \".hidden:1\"\n", "env.image"),
        ("[env]\nimage = \"_x\"\n", "env.image"),
        ("[env]\nimage = \"node 22\"\n", "env.image"),
        ("[env]\nimage = \"node=22\"\n", "env.image"),
        ("[env]\ninstall = []\n", "env.install"),
        ("[limits]\npids = 0\n", "limits.pids"),
        ("[limits]\ncpus = 0\n", "limits.cpus"),
        ("[limits]\ntimeout_s = 5000000\n", "limits.timeout_s"),
        ("[limits]\nmemory_mb = -1\n", ""),
        ("[envv]\nimage = \"x\"\n", "unknown field"),
        ("[env]\nimage = 3\n", ""),
        ("this is not toml", ""),
    ];
    for (toml, needle) in cases {
        let toml = toml.replace("{}", &"a".repeat(64));
        let message = config(&toml);
        assert!(
            message.contains(needle),
            "config {toml:?} failed with {message:?}, expected it to mention {needle:?}"
        );
    }
}

#[test]
fn tree_env_is_the_workspace_resolver() {
    use aver::workspace::EnvResolver;
    let (_dir, store) = open();
    let root = tree(&store, &[("aver.toml", "[env]\nimage = \"node:22\"\n")]);
    let images = StaticResolver::default().with("node:22", digest("cd"));
    let resolver = env::TreeEnv(std::sync::Arc::new(images), env::Detection::Off);
    let base = env::environment(
        &store,
        &tree(&store, &[("a", "b")]),
        &StaticResolver::default(),
        env::Detection::Off,
    )
    .unwrap();
    let env = resolver.resolve(&store, &root, &base).unwrap();
    assert_eq!(env.image.digest, digest("cd"));
    let unresolvable = tree(&store, &[("aver.toml", "[env]\nimage = \"python:3\"\n")]);
    let err = resolver.resolve(&store, &unresolvable, &base).unwrap_err();
    assert!(err.to_string().contains("python:3"), "{err}");
}

#[test]
fn config_must_be_a_regular_file() {
    let message = invalid(&[("aver.toml/inner", "x")]);
    assert!(message.contains("regular file"), "{message}");
}

#[test]
fn environment_ids_depend_only_on_content() {
    let files = [
        ("aver.toml", "[limits]\npids = 64\n"),
        ("package-lock.json", "{}"),
    ];
    let a = resolve(&files).unwrap();
    let b = resolve(&files).unwrap();
    assert_eq!(a.id(), b.id());
    let c = resolve(&[
        ("aver.toml", "[limits]\npids = 65\n"),
        ("package-lock.json", "{}"),
    ])
    .unwrap();
    assert_ne!(a.id(), c.id());
}

#[test]
fn read_file_and_lookup_walk_nested_trees() {
    let (_dir, store) = open();
    let root = tree(&store, &[("a/b/c.txt", "nested"), ("top", "t")]);
    assert_eq!(
        env::read_file(&store, &root, &p("a/b/c.txt")).unwrap(),
        Some(b"nested".to_vec())
    );
    assert_eq!(env::read_file(&store, &root, &p("a/b")).unwrap(), None);
    assert_eq!(env::lookup(&store, &root, &p("top/x")).unwrap(), None);
    assert!(matches!(
        env::lookup(&store, &root, &p("a")).unwrap(),
        Some(Node::Tree(_))
    ));
}

#[test]
fn declared_test_paths_make_a_v1_environment() {
    let env = resolve_with(
        &[(
            "aver.toml",
            "[env]\nrebuild = false\ntests = [\"spec/**\", \"**/*.spec.ts\"]\n",
        )],
        env::Detection::Off,
    )
    .unwrap();
    assert_eq!(env.schema, EnvSchema::V1);
    let tests: Vec<&str> = env.tests.iter().map(|g| g.as_str()).collect();
    assert_eq!(tests, ["**/*.spec.ts", "spec/**"]);
    assert!(env.is_test_file("spec/a.js") && !env.is_test_file("test/a.test.js"));

    let env = resolve_with(
        &[("aver.toml", "[env]\nrebuild = false\n")],
        env::Detection::Off,
    )
    .unwrap();
    assert_eq!(env.schema, EnvSchema::V0);
    assert!(env.is_test_file("test/a.test.js"));
    let env = resolve_with(
        &[("aver.toml", "[env]\ntests = [\"spec/**\"]\n")],
        env::Detection::Off,
    )
    .unwrap();
    assert_eq!(env.schema, EnvSchema::V2, "the default rebuild makes it v2");

    assert!(config("[env]\ntests = []\n").contains("env.tests"));
    assert!(config("[env]\ntests = [\"../x\"]\n").contains("env.tests"));
}

#[test]
fn setup_commands_report_formats_and_rebuild_parse_into_a_v2_environment() {
    let env = resolve(&[(
        "aver.toml",
        r#"[env]
rebuild = false

[[check]]
id = "unit"
run = ["npx", "jest", "--json", "--outputFile=/aver/out/unit.json"]
setup = [["npm", "run", "build"], ["node", "scripts/gen.js"]]
report = { format = "jest-json", path = "/aver/out/unit.json" }

[[check]]
id = "legacy"
run = ["npx", "mocha", "--reporter", "xunit", "--reporter-option", "output=/aver/out/legacy.xml"]
report = { format = "mocha-xunit", path = "/aver/out/legacy.xml" }
"#,
    )])
    .unwrap();
    assert_eq!(env.schema, EnvSchema::V2);
    assert!(!env.install.rebuild);
    let unit = env.checks.get("unit").unwrap();
    assert_eq!(
        unit.setup,
        [
            vec!["npm".to_owned(), "run".into(), "build".into()],
            vec!["node".to_owned(), "scripts/gen.js".into()],
        ]
    );
    assert_eq!(unit.report.as_ref().unwrap().format, ReportFormat::JestJson);
    assert_eq!(
        env.checks
            .get("legacy")
            .unwrap()
            .report
            .as_ref()
            .unwrap()
            .format,
        ReportFormat::MochaXunit
    );
    let allowed =
        resolve(&[("aver.toml", "[env]\nallow_scripts = true\nrebuild = true\n")]).unwrap();
    assert!(
        !allowed.install.rebuild,
        "an install that runs scripts has nothing to rebuild"
    );
    let config = |toml: &str| resolve(&[("aver.toml", toml)]).unwrap_err().to_string();
    let check = "[[check]]\nid = \"u\"\nrun = [\"x\"]\n";
    assert!(config(&format!("{check}setup = []\n")).contains("check u: setup"));
    assert!(config(&format!("{check}setup = [[]]\n")).contains("check u: setup"));
    assert!(config(&format!(
        "{check}report = {{ format = \"xunit\", path = \"/aver/out/x\" }}\n"
    ))
    .contains("report.format"));
}

#[test]
fn the_default_check_is_read_from_package_json_and_the_lockfile() {
    let lock =
        r#"{"lockfileVersion": 3, "packages": {"node_modules/vitest": {"version": "3.2.4"}}}"#;
    let env = resolve(&[
        (
            "package.json",
            r#"{"main": "dist/index.js", "scripts": {"test": "vitest run", "build": "tsc"}}"#,
        ),
        ("package-lock.json", lock),
        (
            "src/index.ts",
            "export {};
",
        ),
    ])
    .unwrap();
    let check = env.checks.get("test").unwrap();
    assert_eq!(
        check.argv,
        [
            "npm",
            "test",
            "--",
            "--reporter=default",
            "--reporter=junit",
            "--outputFile.junit=/aver/out/test.xml"
        ]
    );
    assert_eq!(check.report.as_ref().unwrap().format, ReportFormat::Junit);
    assert_eq!(check.setup, [["npm", "run", "build"]]);
    assert_eq!(check.timeout_ms, Some(600_000));

    let committed = resolve(&[
        (
            "package.json",
            r#"{"main": "dist/index.js", "scripts": {"test": "vitest run", "build": "tsc"}}"#,
        ),
        ("package-lock.json", lock),
        (
            "dist/index.js",
            "module.exports = {};
",
        ),
    ])
    .unwrap();
    assert!(committed.checks.get("test").unwrap().setup.is_empty());

    let custom = resolve(&[(
        "package.json",
        r#"{"scripts": {"test": "node run-tests.js"}}"#,
    )])
    .unwrap();
    let check = custom.checks.get("test").unwrap();
    assert_eq!(check.argv, ["npm", "test"]);
    assert!(check.report.is_none());
    assert_eq!(custom.checks.len(), 1, "no TypeScript, no type check");
}

/// A base that lists `typescript` as a dev dependency beside a
/// `tsconfig.json` also gets the `typecheck` gate, one command-level check.
#[test]
fn a_typescript_base_gets_a_typecheck_check() {
    let manifest =
        r#"{"scripts": {"test": "node --test"}, "devDependencies": {"typescript": "^5.6.0"}}"#;
    let env = resolve(&[("package.json", manifest), ("tsconfig.json", "{}\n")]).unwrap();
    let ids: Vec<&str> = env.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["test", "typecheck"]);
    let check = env.checks.get("typecheck").unwrap();
    assert!(check.argv[2].starts_with("npx tsc --noEmit -p . > /aver/out/typecheck.txt"));
    assert_eq!(
        check.report.as_ref().map(|r| r.format),
        Some(ReportFormat::Tsc)
    );

    let without = resolve(&[("package.json", manifest)]).unwrap();
    assert!(
        without.checks.get("typecheck").is_none(),
        "no tsconfig.json"
    );
}

fn layer_paths(env: &Environment, id: &str) -> Vec<String> {
    env.check_layers(env.checks.get(id).unwrap())
        .iter()
        .map(|l| l.lockfile.path.to_string())
        .collect()
}

/// Generic mode: without `[[check]]`, every Python, Rust, Go and nested
/// Node package gets its own check in its own image with the layer of its
/// lockfile, and the root `package.json` keeps the default `test` check.
#[test]
fn generic_mode_gives_each_package_its_check_image_and_layer() {
    let manifest = r#"{"scripts": {"test": "node --test"}}"#;
    let env = resolve(&[
        ("package.json", manifest),
        (
            "package-lock.json",
            r#"{"lockfileVersion": 3, "packages": {}}"#,
        ),
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/core\"]\n"),
        ("Cargo.lock", "version = 4\n"),
        ("crates/core/Cargo.toml", "[package]\nname = \"core\"\n"),
        ("crates/core/src/lib.rs", ""),
        ("tools/go.mod", "module example.com/tools\n\ngo 1.22\n"),
        ("tools/go.sum", ""),
        ("tools/main_test.go", "package main\n"),
        ("py/pyproject.toml", "[project]\nname = \"py\"\n"),
        ("py/requirements-dev.txt", "pytest==9.1.1\n"),
        ("py/src/py/__init__.py", ""),
        ("py/tests/test_py.py", ""),
        ("web/package.json", manifest),
        (
            "web/package-lock.json",
            r#"{"lockfileVersion": 3, "packages": {}}"#,
        ),
        ("fixtures/demo/go.mod", "module demo\n"),
        ("docs/README.md", "no package here"),
    ])
    .unwrap();
    assert_eq!(env.schema, EnvSchema::V4);
    let ids: Vec<&str> = env.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["cargo", "go-tools", "pytest-py", "test", "test-web"]);
    let approved: Vec<&str> = env
        .checks
        .iter()
        .filter(|c| c.approved)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(
        approved,
        ["test", "test-web"],
        "only the commands a package.json declares"
    );
    assert_eq!(
        env.image.reference,
        env::DEFAULT_IMAGE_REF,
        "the root is Node"
    );

    let cargo = env.checks.get("cargo").unwrap();
    assert_eq!(env.check_image(cargo).reference, "rust:1.99-bookworm");
    assert_eq!(cargo.report.as_ref().unwrap().format, ReportFormat::Libtest);
    assert!(cargo.argv[2].contains("cargo test --workspace --no-fail-fast --locked"));
    assert_eq!(layer_paths(&env, "cargo"), ["Cargo.lock"]);
    assert_eq!(env.limits.memory_mb, 4096, "a Rust build needs room");

    let go = env.checks.get("go-tools").unwrap();
    assert_eq!(go.cwd.as_ref().map(|c| c.as_str()), Some("tools"));
    assert_eq!(
        go.setup,
        [vec!["go".to_owned(), "vet".into(), "./...".into()]]
    );
    assert_eq!(go.report.as_ref().unwrap().format, ReportFormat::GoJson);
    assert_eq!(layer_paths(&env, "go-tools"), ["tools/go.sum"]);

    let py = env.checks.get("pytest-py").unwrap();
    assert_eq!(&py.argv[..4], ["env", "PYTHONPATH=src", "python", "-m"]);
    assert_eq!(
        py.report.as_ref().unwrap().format,
        ReportFormat::PytestJunit
    );
    assert_eq!(layer_paths(&env, "pytest-py"), ["py/requirements-dev.txt"]);
    let pip = env.layers.get("py/requirements-dev.txt").unwrap();
    assert_eq!(pip.kind, LayerKind::Pip);

    assert_eq!(
        layer_paths(&env, "test"),
        ["package-lock.json", "web/package-lock.json"],
        "Node checks link every npm layer"
    );
    let web = env.checks.get("test-web").unwrap();
    assert_eq!(web.cwd.as_ref().map(|c| c.as_str()), Some("web"));
    assert_eq!(
        web.report.as_ref().unwrap().path.as_str(),
        "/aver/out/test-web.xml"
    );
    assert!(env
        .checks
        .iter()
        .all(|c| !c.id.as_str().contains("fixtures")));
}

/// Generic mode's Node packages get the type-check gate the root gets:
/// one per package that lists `typescript` beside its own `tsconfig.json`,
/// in its directory, with the npm layers every Node check links.
#[test]
fn generic_mode_gives_each_typescript_package_its_typecheck() {
    let ts = r#"{"scripts": {"test": "node --test"}, "devDependencies": {"typescript": "^5.6.0"}}"#;
    let lock = r#"{"lockfileVersion": 3, "packages": {}}"#;
    let env = resolve(&[
        ("package.json", ts),
        ("package-lock.json", lock),
        ("tsconfig.json", "{}\n"),
        ("web/package.json", ts),
        ("web/package-lock.json", lock),
        ("web/tsconfig.json", "{}\n"),
        ("api/package.json", ts),
        ("api/package-lock.json", lock),
        ("go.mod", "module example.com/x\n"),
    ])
    .unwrap();
    let ids: Vec<&str> = env.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "go",
            "test",
            "test-api",
            "test-web",
            "typecheck",
            "typecheck-web"
        ],
        "api has no tsconfig.json, so no gate"
    );
    let web = env.checks.get("typecheck-web").unwrap();
    assert_eq!(web.cwd.as_ref().map(|c| c.as_str()), Some("web"));
    assert!(web.argv[2].starts_with("npx tsc --noEmit -p . > /aver/out/typecheck-web.txt"));
    assert_eq!(
        web.report.as_ref().map(|r| r.format),
        Some(ReportFormat::Tsc)
    );
    assert!(web.image.is_none());
    assert_eq!(
        layer_paths(&env, "typecheck-web"),
        layer_paths(&env, "test-web")
    );
    assert!(env.checks.get("typecheck").unwrap().cwd.is_none());

    // Without a root package.json the environment's image is the first
    // package's, and the Node packages' checks carry the Node image.
    let env = resolve(&[
        ("go.mod", "module example.com/x\n"),
        ("web/package.json", ts),
        ("web/package-lock.json", lock),
        ("web/tsconfig.json", "{}\n"),
    ])
    .unwrap();
    let gate = env.checks.get("typecheck-web").unwrap();
    assert_eq!(env.check_image(gate).reference, env::DEFAULT_IMAGE_REF);
}

/// WP6 JS-6: a pnpm monorepo whose root has no `test` script (date-fns)
/// gets a check per workspace package with a `test` script, read per test
/// from its own `package.json`, and no root `npm test` that cannot run.
#[test]
fn a_workspace_roots_packages_get_their_own_checks() {
    let vitest = r#"{"scripts": {"test": "vitest run"}, "devDependencies": {"vitest": "^3.2.0"}}"#;
    let env = resolve(&[
        (
            "package.json",
            r#"{"private": true, "devDependencies": {}}"#,
        ),
        ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
        (
            "pnpm-workspace.yaml",
            "packages:\n  - 'pkgs/*'\n  - \"!pkgs/ignored\"\ncatalog:\n  - x\n",
        ),
        ("pkgs/core/package.json", vitest),
        ("pkgs/ignored/package.json", vitest),
        ("pkgs/core/examples/package.json", vitest),
        ("tools/package.json", vitest),
    ])
    .unwrap();
    let ids: Vec<&str> = env.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["test-pkgs-core"]);
    let core = env.checks.get("test-pkgs-core").unwrap();
    assert_eq!(core.cwd.as_ref().map(|c| c.as_str()), Some("pkgs/core"));
    assert_eq!(
        core.report.as_ref().map(|r| r.format),
        Some(ReportFormat::Junit),
        "{:?}",
        core.argv
    );

    // npm and Yarn workspaces, in the root package.json.
    let env = resolve(&[
        (
            "package.json",
            r#"{"workspaces": {"packages": ["packages/*"]}}"#,
        ),
        ("yarn.lock", "\n"),
        (
            "packages/a/package.json",
            r#"{"scripts": {"test": "node --test"}}"#,
        ),
    ])
    .unwrap();
    let ids: Vec<&str> = env.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["test-packages-a"]);
}

/// WP6 JS-9: a workspace package has no install of its own, so a runner
/// only the root's `package.json` or `package-lock.json` lists (hono's
/// adapters run `vp test`, with `vite-plus` at the root) is read per test.
#[test]
fn a_workspace_package_reads_the_runner_the_root_installs() {
    let adapter = r#"{"scripts": {"test": "tsc -p tsconfig.json && vp test --run"}}"#;
    let env = resolve(&[
        (
            "package.json",
            r#"{"private": true, "devDependencies": {"vite-plus": "0.1.16"}}"#,
        ),
        ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
        ("pnpm-workspace.yaml", "packages:\n  - 'adapters/*'\n"),
        ("adapters/bun/package.json", adapter),
    ])
    .unwrap();
    let check = env.checks.get("test-adapters-bun").unwrap();
    assert_eq!(
        check.report.as_ref().map(|r| r.format),
        Some(ReportFormat::Junit),
        "{:?}",
        check.argv
    );

    let lock =
        r#"{"lockfileVersion": 3, "packages": {"node_modules/vitest": {"version": "3.2.4"}}}"#;
    let env = resolve(&[
        (
            "package.json",
            r#"{"private": true, "workspaces": ["packages/*"]}"#,
        ),
        ("package-lock.json", lock),
        (
            "packages/a/package.json",
            r#"{"scripts": {"test": "vitest run"}}"#,
        ),
    ])
    .unwrap();
    let check = env.checks.get("test-packages-a").unwrap();
    assert_eq!(
        check.report.as_ref().map(|r| r.format),
        Some(ReportFormat::Junit),
        "{:?}",
        check.argv
    );
}

#[test]
fn generic_mode_check_ids_stay_unique() {
    let env = resolve(&[("a-b/go.mod", "module x\n"), ("a/b/go.mod", "module y\n")]).unwrap();
    let ids: Vec<&str> = env.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["go-a-b", "go-a-b-2"]);
    let second = env.checks.get("go-a-b-2").unwrap();
    assert_eq!(second.cwd.as_ref().map(|c| c.as_str()), Some("a/b"));
    assert_eq!(
        second.report.as_ref().unwrap().path.as_str(),
        "/aver/out/go-a-b-2.json"
    );
    assert!(second.argv[2].contains("/aver/out/go-a-b-2.json"));
}

#[test]
fn generic_mode_leaves_a_node_only_repository_as_it_was() {
    let files = [
        ("package.json", r#"{"scripts": {"test": "node --test"}}"#),
        ("src/index.js", ""),
        ("test/index.test.js", ""),
    ];
    let env = resolve(&files).unwrap();
    assert_ne!(env.schema, EnvSchema::V3);
    assert!(env.layers.is_empty());
    assert_eq!(env.checks.len(), 1);
    let python = resolve(&[
        ("pyproject.toml", "[project]\nname = \"x\"\n"),
        ("test_x.py", ""),
    ])
    .unwrap();
    let check = python.checks.iter().next().unwrap();
    assert_eq!(
        check.id.as_str(),
        "unittest",
        "no pytest installed, no report"
    );
    assert!(check.report.is_none() && check.image.is_none());
    assert_eq!(python.image.reference, "python:3.14-slim-bookworm");
}

/// pytest is detected from any mention in the project, lockfile or not
/// (tqdm, packaging and black declare it in `pyproject.toml` alone), and
/// generic mode adds no type check to it (click, attrs, flask and typer
/// name `mypy` in a group the test job does not install).
#[test]
fn generic_mode_detects_pytest_without_a_lockfile_and_adds_no_type_check() {
    let pyproject = |extra: &str| format!("[project]\nname = \"x\"\n{extra}");
    for (name, files) in [
        (
            "ini options",
            vec![(
                "pyproject.toml",
                pyproject("\n[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n"),
            )],
        ),
        (
            "dependency group",
            vec![(
                "pyproject.toml",
                pyproject("\n[dependency-groups]\ntest = [\"pytest>=8\", \"pretend\"]\n"),
            )],
        ),
        (
            "optional extra",
            vec![(
                "pyproject.toml",
                pyproject("\n[project.optional-dependencies]\ntests = [\"pytest\"]\n"),
            )],
        ),
        (
            "tox",
            vec![
                ("pyproject.toml", pyproject("")),
                (
                    "tox.ini",
                    "[testenv]\ncommands = pytest {posargs}\n".to_owned(),
                ),
            ],
        ),
        (
            "nox",
            vec![
                ("pyproject.toml", pyproject("")),
                ("noxfile.py", "session.run(\"pytest\")\n".to_owned()),
            ],
        ),
        (
            "setup.cfg",
            vec![
                ("setup.cfg", "[tool:pytest]\naddopts = -ra\n".to_owned()),
                ("setup.py", String::new()),
            ],
        ),
        (
            "conftest",
            vec![
                ("pyproject.toml", pyproject("")),
                ("conftest.py", String::new()),
            ],
        ),
        (
            "mypy configured and listed",
            vec![
                (
                    "pyproject.toml",
                    pyproject("\n[tool.mypy]\nfiles = [\"src\"]\n\n[tool.pytest.ini_options]\n"),
                ),
                ("uv.lock", "name = \"mypy\"\nname = \"pytest\"\n".to_owned()),
            ],
        ),
    ] {
        let mut files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
        files.push(("tests/test_x.py", "def test_x(): pass\n"));
        let env = resolve(&files).unwrap();
        let check = env.checks.iter().next().unwrap();
        assert_eq!(check.id.as_str(), "pytest", "{name}");
        assert!(check.report.is_some(), "{name}");
        assert!(check.setup.is_empty(), "{name}: {:?}", check.setup);
    }
}

/// The detected pytest check leaves the project's own options to pytest:
/// `addopts` such as `-n auto` or `-k "not perf"` apply as they do in the
/// project's CI, and Aver only adds its report options.
#[test]
fn generic_mode_keeps_the_project_s_pytest_options() {
    let pyproject = "[project]\nname = \"x\"\n\n[dependency-groups]\ntests = [\"pytest\", \"pytest-xdist\"]\n\n[tool.pytest.ini_options]\naddopts = \"-n auto -k 'not perf'\"\n";
    let workflow = "jobs:\n  t:\n    steps:\n      - run: uv run pytest -n auto tests\n";
    let env = resolve(&[
        ("pyproject.toml", pyproject),
        (".github/workflows/ci.yml", workflow),
        ("tests/test_x.py", "def test_x(): pass\n"),
    ])
    .unwrap();
    let check = env.checks.iter().next().unwrap();
    assert_eq!(
        check.argv,
        [
            "python",
            "-m",
            "pytest",
            "-p",
            "no:cacheprovider",
            "--continue-on-collection-errors",
            "-o",
            "junit_family=xunit1",
            "--junitxml=/aver/out/pytest.xml",
        ]
    );
}

#[test]
fn deps_tables_declare_layers_and_checks_mount_them_by_family_and_cwd() {
    let toml = format!(
        r#"[env]
image = "node:22@sha256:{node}"
lockfile = "package-lock.json"

[[deps]]
lockfile = "Cargo.lock"

[[deps]]
lockfile = "svc/uv.lock"

[[deps]]
lockfile = "site/package-lock.json"

[[check]]
id = "rust"
image = "rust:1.99-bookworm@sha256:{rust}"
run = ["cargo", "test"]
report = {{ format = "libtest", path = "/aver/out/rust.txt" }}
covers = ["**/*.rs"]

[[check]]
id = "svc"
image = "python:3.14-slim-bookworm@sha256:{rust}"
cwd = "svc/api"
run = ["python", "-m", "pytest"]

[[check]]
id = "web"
cwd = "site"
run = ["npm", "test"]

[[check]]
id = "only-mine"
run = ["npm", "test"]
deps = ["site/package-lock.json"]
"#,
        node = "ab".repeat(32),
        rust = "cd".repeat(32),
    );
    let env = resolve(&[
        ("aver.toml", &toml),
        ("package-lock.json", "{}"),
        ("Cargo.lock", "version = 4\n"),
        ("svc/uv.lock", "version = 1\n"),
        ("svc/pyproject.toml", ""),
        ("site/package-lock.json", "{}"),
    ])
    .unwrap();
    assert_eq!(env.schema, EnvSchema::V3);
    assert_eq!(layer_paths(&env, "rust"), ["Cargo.lock"]);
    assert_eq!(layer_paths(&env, "svc"), ["svc/uv.lock"]);
    assert_eq!(env.layers.get("svc/uv.lock").unwrap().kind, LayerKind::Uv);
    assert_eq!(
        layer_paths(&env, "web"),
        ["package-lock.json", "site/package-lock.json"]
    );
    assert_eq!(layer_paths(&env, "only-mine"), ["site/package-lock.json"]);
    assert_eq!(
        env.layers.get("Cargo.lock").unwrap().install.argv,
        ["cargo", "fetch", "--locked"]
    );

    for (bad, why) in [
        ("[[deps]]\nlockfile = \"missing.lock\"\n", "not a file"),
        (
            "[[deps]]\nlockfile = \"package-lock.json\"\n",
            "declared twice",
        ),
        ("[[deps]]\nlockfile = \"weird.lock\"\n", "no kind"),
        (
            "[[deps]]\nlockfile = \"Cargo.lock\"\nrebuild = true\n",
            "only npm layers rebuild",
        ),
        (
            "[[check]]\nid = \"x\"\nrun = [\"x\"]\ndeps = [\"nope.lock\"]\n",
            "neither env.lockfile",
        ),
    ] {
        let message = invalid(&[
            ("aver.toml", bad),
            ("package-lock.json", "{}"),
            ("Cargo.lock", "version = 4\n"),
            ("weird.lock", ""),
        ]);
        assert!(message.contains(why), "{why}: {message}");
    }
}

/// Affected checks (§1.9.3): a change affects a check when it is in the
/// check's scope (its `covers`, else anything under its `cwd`), a lockfile
/// of a layer it mounts, `aver.toml`, or harness on the way to or under its
/// directory.
#[test]
fn a_change_affects_the_checks_whose_scope_layers_or_harness_it_touches() {
    let toml = format!(
        r#"[env]
image = "node:22@sha256:{node}"
lockfile = "package-lock.json"

[[deps]]
lockfile = "Cargo.lock"

[[deps]]
lockfile = "site/package-lock.json"

[[check]]
id = "rust"
image = "rust:1.99-bookworm@sha256:{rust}"
run = ["cargo", "test"]
covers = ["crates/*/src/**"]

[[check]]
id = "web"
cwd = "site"
run = ["npm", "test"]

[[check]]
id = "root"
run = ["npm", "test"]
deps = []
"#,
        node = "ab".repeat(32),
        rust = "cd".repeat(32),
    );
    let env = resolve(&[
        ("aver.toml", &toml),
        ("package-lock.json", "{}"),
        ("Cargo.lock", "version = 4\n"),
        ("site/package-lock.json", "{}"),
    ])
    .unwrap();
    let dirs = aver::judging::harness_dirs(env.checks.iter().map(|c| c.cwd.as_ref()));
    let affected = |path: &str| -> Vec<&str> {
        env.checks
            .iter()
            .filter(|c| aver::coverage::affects(&env, &dirs, c, path, true))
            .map(|c| c.id.as_str())
            .collect()
    };
    // A root check without covers, as in a single-package repository, is
    // affected by everything.
    assert_eq!(affected("docs/guide.md"), ["root"]);
    assert_eq!(affected("crates/core/src/lib.rs"), ["root", "rust"]);
    assert_eq!(affected("site/src/app.ts"), ["root", "web"]);
    // Every check runs under the configuration.
    assert_eq!(affected("aver.toml"), ["root", "rust", "web"]);
    // A lockfile affects the checks that mount its layer, and the root
    // check, whose scope it is in.
    assert_eq!(affected("Cargo.lock"), ["root", "rust"]);
    assert_eq!(affected("site/package-lock.json"), ["root", "web"]);
    // Harness: a crate's manifest is under the rust check's root
    // directory; a package manifest at the root is on the way to site/.
    assert_eq!(affected("crates/core/Cargo.toml"), ["root", "rust"]);
    assert_eq!(affected("package.json"), ["root", "rust", "web"]);
    assert_eq!(affected("site/tsconfig.json"), ["root", "web"]);
}

#[test]
fn without_a_contract_nothing_is_detected_outside_ci() {
    let files = [
        ("package.json", r#"{"scripts": {"test": "node --test"}}"#),
        ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ("Cargo.lock", "version = 4\n"),
    ];
    let local = resolve_with(&files, env::Detection::Off).unwrap();
    assert!(local.checks.is_empty(), "{:?}", local.checks);
    let ci = resolve(&files).unwrap();
    assert!(!ci.checks.is_empty());
    assert!(
        ci.checks.get("test").unwrap().approved,
        "the package's own script"
    );
    assert!(
        !ci.checks.get("cargo").unwrap().approved,
        "Aver's own command"
    );
    let named = resolve(&[(
        "aver.toml",
        "[[check]]\nid = \"unit\"\nrun = [\"node\", \"--test\"]\n",
    )])
    .unwrap();
    assert!(
        named.checks.iter().all(|c| c.approved),
        "named in aver.toml"
    );
}

#[test]
fn a_contract_line_that_sets_variables_runs_under_a_shell() {
    let line = "PYTHONPATH=src python3 -m pytest --junitxml=$AVER_OUT/py.xml";
    let contract =
        format!("[[check]]\nid = \"py\"\nrun = \"{line}\"\nreport = \"junit\"\nmin_tests = 1\n");
    let env = resolve_with(&[("aver.toml", &contract)], env::Detection::Off).unwrap();
    assert_eq!(
        env.checks.get("py").unwrap().argv,
        [
            "sh",
            "-c",
            "PYTHONPATH=src python3 -m pytest --junitxml=/aver/out/py.xml"
        ]
    );
}

#[test]
fn a_contract_loads_into_the_checks_it_names() {
    let contract = r#"
[[check]]
id = "unit"
run = "node --test --test-reporter=junit --test-reporter-destination=$AVER_OUT/node.xml"
min_tests = 40

[[check]]
id = "rust"
run = "cargo test --workspace"
report = "libtest"
min_tests = 1
paths = ["crates/**"]
timeout_s = 900

[[check]]
id = "lint"
run = "npm run lint && npm run fmt"
report = "exit"
prepare = "npm run build"

[deps]
keep = ["node_modules", "target"]
inputs = ["package-lock.json", "Cargo.lock"]
"#;
    let env = resolve_with(
        &[
            ("aver.toml", contract),
            ("package-lock.json", "{}"),
            ("Cargo.lock", "version = 4\n"),
        ],
        env::Detection::Off,
    )
    .unwrap();
    assert_eq!(env.schema, EnvSchema::V4, "min_tests is a v4 field");
    let unit = env.checks.get("unit").unwrap();
    assert_eq!(
        unit.argv,
        [
            "node",
            "--test",
            "--test-reporter=junit",
            "--test-reporter-destination=/aver/out/node.xml"
        ],
        "a single command runs as its words"
    );
    let report = unit.report.as_ref().unwrap();
    assert_eq!(
        (report.format, report.path.as_str()),
        (ReportFormat::Junit, "/aver/out/node.xml"),
        "the one JUnit file the line writes"
    );
    assert_eq!(unit.floor().map(|n| n.get()), Some(40));
    assert!(unit.approved);

    let rust = env.checks.get("rust").unwrap();
    assert_eq!(
        rust.argv,
        [
            "sh",
            "-c",
            "( cargo test --workspace ) >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s"
        ]
    );
    assert_eq!(rust.report.as_ref().unwrap().format, ReportFormat::Libtest);
    assert_eq!(rust.floor().map(|n| n.get()), Some(1));
    assert_eq!(rust.timeout_ms, Some(900_000));
    assert_eq!(
        rust.covers.iter().map(|g| g.as_str()).collect::<Vec<_>>(),
        ["crates/**"]
    );
    assert_eq!(
        env.check_image(rust).reference,
        "rust:1.99-bookworm",
        "a cargo command covers Rust files"
    );
    assert_eq!(
        env.check_layers(rust)
            .iter()
            .map(|l| l.lockfile.path.as_str())
            .collect::<Vec<_>>(),
        ["Cargo.lock"],
        "the Cargo lockfile among deps.inputs is its layer"
    );

    let lint = env.checks.get("lint").unwrap();
    assert_eq!(lint.argv, ["sh", "-c", "npm run lint && npm run fmt"]);
    assert!(lint.report.is_none() && lint.floor().is_none());
    assert_eq!(lint.setup, [["sh", "-c", "npm run build"]]);

    let bad = resolve_with(
        &[(
            "aver.toml",
            "[[check]]\nid = \"unit\"\nrun = \"npm test\"\nreport = \"exit\"\nmin_tests = 3\n",
        )],
        env::Detection::Off,
    )
    .unwrap_err()
    .to_string();
    assert!(bad.contains("min_tests"), "{bad}");
}
