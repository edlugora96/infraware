//! Path-only technology pattern detection (mvp2-technology-pattern-detection).
//!
//! Pure predicates over normalized repository-relative forward-slash paths.
//! No file contents are read. All functions are `pub(crate)` as this is an
//! internal analysis crate (Chapter 2: restrict visibility to `pub(crate)`).

use std::collections::{BTreeMap, BTreeSet};

/// Known conventional structure directory names.
pub(crate) const STRUCTURE_DIRS: &[&str] = &[
    "src", "app", "lib", "cmd", "internal", "apps", "packages", "services",
];

/// Workspace config file basenames (path-only detection).
pub(crate) const WORKSPACE_FILES: &[&str] =
    &["pnpm-workspace.yaml", "lerna.json", "nx.json", "turbo.json"];

/// Grouped technology evidence collected in a single walk.
#[derive(Debug, Clone, Default)]
pub(crate) struct CollectedPatterns {
    /// Conventional structure directories found.
    pub(crate) structure: BTreeSet<String>,
    /// CI/CD system -> evidence paths.
    pub(crate) cicd: BTreeMap<String, BTreeSet<String>>,
    /// Infrastructure tool -> evidence paths.
    pub(crate) infra: BTreeMap<String, BTreeSet<String>>,
    /// DB/ORM marker -> evidence paths.
    pub(crate) db: BTreeMap<String, BTreeSet<String>>,
    /// Whether any workspace config was found.
    pub(crate) workspace_found: bool,
    /// Whether any package-group directory was found.
    pub(crate) group_dir_found: bool,
    /// Distinct directories containing manifests.
    pub(crate) manifest_dirs: BTreeSet<String>,
}

/// Returns the matched structure directory name if `rel` contains a known
/// conventional segment; otherwise `None`.
///
/// Borrows `rel` as `&str` (Chapter 1: prefer `&str` over `String` in
/// parameters) and avoids cloning unless a match is found.
pub(crate) fn match_structure(rel: &str, is_dir: bool) -> Option<String> {
    // Cheap segment check without allocating intermediate Vec where possible:
    // iterate over split segments and return owned String only on match.
    for seg in rel.split('/') {
        if STRUCTURE_DIRS.contains(&seg) {
            return Some(seg.to_string());
        }
    }
    if is_dir {
        if let Some(name) = rel.rsplit('/').next() {
            if STRUCTURE_DIRS.contains(&name) {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Classify CI/CD system from a path; `None` if ambiguous/unsupported.
pub(crate) fn match_cicd(rel: &str) -> Option<String> {
    let lower = rel.to_lowercase();
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    if rel.contains(".github/workflows/") && (lower.ends_with(".yml") || lower.ends_with(".yaml")) {
        return Some("github-actions".to_string());
    }
    if name == ".gitlab-ci.yml" || lower.ends_with("/.gitlab-ci.yml") {
        return Some("gitlab-ci".to_string());
    }
    if rel.contains(".circleci/") && (name == "config.yml" || name == "config.yaml") {
        return Some("circleci".to_string());
    }
    if name.starts_with("jenkinsfile") {
        let base = rel.rsplit('/').next().unwrap_or(rel);
        if base.starts_with("Jenkinsfile") {
            return Some("jenkins".to_string());
        }
    }
    if name == "azure-pipelines.yml" || name == "azure-pipelines.yaml" {
        return Some("azure-pipelines".to_string());
    }
    if rel.contains(".buildkite/") && (name == "pipeline.yml" || name == "pipeline.yaml") {
        return Some("buildkite".to_string());
    }
    None
}

/// Classify infrastructure tool from a path; `None` if ambiguous.
pub(crate) fn match_infra(rel: &str) -> Option<String> {
    let lower = rel.to_lowercase();
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    if name.starts_with("dockerfile") {
        return Some("docker".to_string());
    }
    if name.starts_with("docker-compose.") && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
    {
        return Some("docker-compose".to_string());
    }
    if name == "compose.yml" || name == "compose.yaml" {
        return Some("docker-compose".to_string());
    }
    if (rel.starts_with("k8s/")
        || rel.contains("/k8s/")
        || rel.starts_with("manifests/")
        || rel.contains("/manifests/"))
        && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
    {
        return Some("kubernetes".to_string());
    }
    if name == "chart.yaml" {
        return Some("helm".to_string());
    }
    if name == "kustomization.yaml" || name == "kustomization.yml" {
        return Some("kustomize".to_string());
    }
    if name == "tiltfile" {
        return Some("tilt".to_string());
    }
    if lower.ends_with(".tf") {
        return Some("terraform".to_string());
    }
    None
}

/// Classify DB/ORM/migration marker from a path.
pub(crate) fn match_db(rel: &str) -> Option<String> {
    let lower = rel.to_lowercase();
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    if rel == "prisma/schema.prisma" || rel.ends_with("/prisma/schema.prisma") {
        return Some("prisma".to_string());
    }
    if name.starts_with("drizzle.config.") {
        return Some("drizzle".to_string());
    }
    if name == "diesel.toml" {
        return Some("diesel".to_string());
    }
    if rel.starts_with(".sqlx/") || rel.contains("/.sqlx/") || rel == ".sqlx" {
        return Some("sqlx".to_string());
    }
    if name == "alembic.ini" {
        return Some("alembic".to_string());
    }
    if rel.contains("alembic/versions/") && lower.ends_with(".py") {
        return Some("alembic".to_string());
    }
    if rel.starts_with("migrations/") || rel.contains("/migrations/") {
        return Some("migrations".to_string());
    }
    None
}

/// Determine project shape from collected evidence.
pub(crate) fn project_shape(
    workspace_found: bool,
    group_dir_found: bool,
    manifest_dirs: &BTreeSet<String>,
) -> String {
    if workspace_found || group_dir_found || manifest_dirs.len() > 1 {
        return "monorepo".to_string();
    }
    "single-project".to_string()
}

/// Whether `rel` is a workspace config file.
pub(crate) fn is_workspace_file(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    WORKSPACE_FILES.contains(&name)
}

/// Whether `rel` is a package-group directory.
pub(crate) fn is_group_dir(rel: &str, is_dir: bool) -> bool {
    if !is_dir {
        return false;
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    matches!(name, "apps" | "packages" | "services")
}

/// Aggregate technology patterns from walked entries in a single O(n) pass.
///
/// Uses iterators without intermediate `collect()` where possible (Chapter 3:
/// zero-cost iterators) and borrows entries as `&[WalkEntry]` (Chapter 1).
pub(crate) fn collect_patterns(
    entries: &[crate::analyze::traversal::WalkEntry],
) -> CollectedPatterns {
    let mut out = CollectedPatterns::default();

    for e in entries {
        if let Some(s) = match_structure(&e.rel, e.is_dir) {
            out.structure.insert(s);
        }
        if let Some(k) = match_cicd(&e.rel) {
            out.cicd.entry(k).or_default().insert(e.rel.clone());
        }
        if let Some(k) = match_infra(&e.rel) {
            out.infra.entry(k).or_default().insert(e.rel.clone());
        }
        if let Some(k) = match_db(&e.rel) {
            out.db.entry(k).or_default().insert(e.rel.clone());
        }
        if is_workspace_file(&e.rel) {
            out.workspace_found = true;
        }
        if is_group_dir(&e.rel, e.is_dir) {
            out.group_dir_found = true;
        }
        let name = e.rel.rsplit('/').next().unwrap_or(&e.rel).to_lowercase();
        let is_manifest = matches!(
            name.as_str(),
            "cargo.toml"
                | "package.json"
                | "go.mod"
                | "pyproject.toml"
                | "pom.xml"
                | "build.gradle"
                | "gemfile"
        );
        if is_manifest {
            let dir = e
                .rel
                .rfind('/')
                .map_or(".", |pos| &e.rel[..pos])
                .to_string();
            out.manifest_dirs.insert(dir);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_detects_known_dirs() {
        assert_eq!(
            match_structure("src/main.rs", false),
            Some("src".to_string())
        );
        assert_eq!(match_structure("apps", true), Some("apps".to_string()));
        assert_eq!(match_structure("other/file.txt", false), None);
        assert_eq!(
            match_structure("internal/foo", false),
            Some("internal".to_string())
        );
    }

    #[test]
    fn cicd_detects_each_marker() {
        assert_eq!(
            match_cicd(".github/workflows/ci.yml"),
            Some("github-actions".to_string())
        );
        assert_eq!(
            match_cicd(".github/workflows/ci.yaml"),
            Some("github-actions".to_string())
        );
        assert_eq!(match_cicd(".gitlab-ci.yml"), Some("gitlab-ci".to_string()));
        assert_eq!(
            match_cicd(".circleci/config.yml"),
            Some("circleci".to_string())
        );
        assert_eq!(match_cicd("Jenkinsfile"), Some("jenkins".to_string()));
        assert_eq!(match_cicd("Jenkinsfile.prod"), Some("jenkins".to_string()));
        assert_eq!(
            match_cicd("azure-pipelines.yml"),
            Some("azure-pipelines".to_string())
        );
        assert_eq!(
            match_cicd(".buildkite/pipeline.yml"),
            Some("buildkite".to_string())
        );
        assert_eq!(match_cicd("some/random.yml"), None);
    }

    #[test]
    fn infra_detects_each_marker() {
        assert_eq!(match_infra("Dockerfile"), Some("docker".to_string()));
        assert_eq!(match_infra("Dockerfile.prod"), Some("docker".to_string()));
        assert_eq!(
            match_infra("docker-compose.yml"),
            Some("docker-compose".to_string())
        );
        assert_eq!(
            match_infra("k8s/deployment.yaml"),
            Some("kubernetes".to_string())
        );
        assert_eq!(
            match_infra("manifests/service.yml"),
            Some("kubernetes".to_string())
        );
        assert_eq!(match_infra("Chart.yaml"), Some("helm".to_string()));
        assert_eq!(
            match_infra("kustomization.yaml"),
            Some("kustomize".to_string())
        );
        assert_eq!(match_infra("Tiltfile"), Some("tilt".to_string()));
        assert_eq!(match_infra("infra/main.tf"), Some("terraform".to_string()));
        assert_eq!(match_infra("random.txt"), None);
    }

    #[test]
    fn db_detects_each_marker() {
        assert_eq!(match_db("prisma/schema.prisma"), Some("prisma".to_string()));
        assert_eq!(match_db("drizzle.config.ts"), Some("drizzle".to_string()));
        assert_eq!(match_db("diesel.toml"), Some("diesel".to_string()));
        assert_eq!(match_db(".sqlx/query.json"), Some("sqlx".to_string()));
        assert_eq!(match_db("alembic.ini"), Some("alembic".to_string()));
        assert_eq!(
            match_db("alembic/versions/abc.py"),
            Some("alembic".to_string())
        );
        assert_eq!(
            match_db("migrations/001_init.sql"),
            Some("migrations".to_string())
        );
        assert_eq!(match_db("random.txt"), None);
    }

    #[test]
    fn shape_monorepo_when_indicators() {
        let mut dirs = BTreeSet::new();
        dirs.insert("a".to_string());
        dirs.insert("b".to_string());
        assert_eq!(project_shape(true, false, &BTreeSet::new()), "monorepo");
        assert_eq!(project_shape(false, true, &BTreeSet::new()), "monorepo");
        assert_eq!(project_shape(false, false, &dirs), "monorepo");
    }

    #[test]
    fn shape_single_when_no_indicators() {
        let mut dirs = BTreeSet::new();
        dirs.insert("a".to_string());
        assert_eq!(project_shape(false, false, &dirs), "single-project");
        assert_eq!(
            project_shape(false, false, &BTreeSet::new()),
            "single-project"
        );
    }
}
