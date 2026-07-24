//! The single owner of Project state + identity (ADR-0028,
//! `docs/components/project.md`). A **Project** is the top-level organizational
//! primitive: a **unique name** (its human key) and a **unique cwd** (its root),
//! plus an extensible **params** bag (empty for now — future per-project config).
//! Workspaces and agent sessions *belong to* a project by holding a
//! [`ProjectId`] **foreign key** and resolve their cwd from it at the point of
//! use (`UXI-Project-2`) — the cwd is never cached on a workspace or session, so
//! there is nothing to keep in sync and nothing to drift.
//!
//! The [`Projects`] store keeps **both** uniqueness invariants **by
//! construction**: `by_name` and `by_cwd` are private and mutated *only* by this
//! module's API, so two projects sharing a name — or a cwd — is unrepresentable
//! (the ownership pattern that made the agent-session 1:1 bugs impossible;
//! `AgentSessions`, ADR-0026). Enforcing cwd-uniqueness makes [`Projects::by_cwd`]
//! **total-or-none**, which is what lets a project-agnostic server session be
//! mapped to its project unambiguously ([`Membership::Inferred`]).

use super::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Stable, monotonic, never-reused local identity for a project. A distinct
/// id-space from [`SessionId`] and the layout `WindowId` (ADR-0026 § "distinct
/// id-spaces get newtypes"). Persisted references use the project's **name**,
/// not this runtime id (ADR-0028 §7).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct ProjectId(pub(crate) u64);

/// One project: a unique name (its human key), a unique cwd (its root), and an
/// extensible params bag. `params` is scaffolded but unpopulated in this pass —
/// future `/new-ux` passes promote a param to a typed field the moment it gains
/// a consumer (the ADR-0023 lesson applies to this primitive on day one); the
/// map stays for genuinely opaque passthrough only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Project {
    pub(crate) name: String,
    pub(crate) cwd: PathBuf,
    pub(crate) params: BTreeMap<String, String>,
}

impl Project {
    fn new(name: String, cwd: PathBuf) -> Self {
        Self {
            name,
            cwd,
            params: BTreeMap::new(),
        }
    }
}

/// Why a `create` was refused — surfaced to the user as a transient error so a
/// name/cwd clash creates nothing (`UXI-Project-1`, `UXI-Project-4`).
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum CreateError {
    /// The name is already taken by another project (the human key is unique).
    DuplicateName,
    /// The cwd is already a project's root (one project per directory).
    DuplicateCwd(ProjectId),
}

/// How a workspace or session is related to a project — the durable answer to
/// "the session server is project-agnostic" (ADR-0028 §3). `Assigned` is the
/// stored foreign key (authoritative); `Inferred` is resolved by cwd for a
/// foreign/roster session that carries no assignment (recomputed, never
/// persisted as authority); `Unfiled` is the honest "no project matches this
/// cwd" state (e.g. a free roster session in a dir with no project).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Membership {
    Assigned(ProjectId),
    Inferred(ProjectId),
    Unfiled(PathBuf),
}

impl Membership {
    /// The project id when known (assigned or inferred), else `None` (unfiled).
    pub(crate) fn project(&self) -> Option<ProjectId> {
        match self {
            Membership::Assigned(id) | Membership::Inferred(id) => Some(*id),
            Membership::Unfiled(_) => None,
        }
    }
}

/// THE owner. Private fields — the app reaches projects only through this API,
/// which is what makes "two projects, one name" and "two projects, one cwd" both
/// unrepresentable.
pub(crate) struct Projects {
    by_id: BTreeMap<ProjectId, Project>,
    /// Private uniqueness index on the human key; only `create`/`rename`/`close`
    /// write it.
    by_name: HashMap<String, ProjectId>,
    /// Private uniqueness index on the **canonical** cwd key (ADR-0010
    /// `cwd_match_key`), so `/tmp` vs `/private/tmp` resolve to one project.
    by_cwd: HashMap<PathBuf, ProjectId>,
    next: u64,
}

impl Default for Projects {
    fn default() -> Self {
        Self {
            by_id: BTreeMap::new(),
            by_name: HashMap::new(),
            by_cwd: HashMap::new(),
            next: 0,
        }
    }
}

impl Projects {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn alloc(&mut self) -> ProjectId {
        let id = ProjectId(self.next);
        self.next += 1;
        id
    }

    /// Create a project. Refuses a duplicate **name** or **cwd** and mutates
    /// nothing on refusal (`UXI-Project-1`). This is the ONLY way to mint one.
    pub(crate) fn create(&mut self, name: String, cwd: PathBuf) -> Result<ProjectId, CreateError> {
        if self.by_name.contains_key(&name) {
            return Err(CreateError::DuplicateName);
        }
        let key = cwd_match_key(&cwd);
        if let Some(&owner) = self.by_cwd.get(&key) {
            return Err(CreateError::DuplicateCwd(owner));
        }
        let id = self.alloc();
        self.by_name.insert(name.clone(), id);
        self.by_cwd.insert(key, id);
        self.by_id.insert(id, Project::new(name, cwd));
        Ok(id)
    }

    /// Create a project rooted at `cwd`, deriving a **unique** name from
    /// `name_hint` (appending ` (2)`, ` (3)`, … on a name clash). If a project
    /// already roots at `cwd`, returns it unchanged. Used by the migration
    /// (`UXI-Project-8`) and self-healing load so distinct dirs never collide on
    /// a shared basename and both uniqueness invariants stay total.
    pub(crate) fn ensure_at_cwd(&mut self, cwd: PathBuf, name_hint: &str) -> ProjectId {
        if let Some(id) = self.by_cwd(&cwd) {
            return id;
        }
        let mut name = name_hint.to_string();
        let mut n = 2;
        while self.by_name.contains_key(&name) {
            name = format!("{name_hint} ({n})");
            n += 1;
        }
        self.create(name, cwd)
            .expect("name uniquified and cwd was absent under the same &mut borrow")
    }

    /// Rename a project, keeping name-uniqueness. Refuses if the new name is
    /// already taken by a *different* project; renaming to the same name is a
    /// no-op success.
    pub(crate) fn rename(&mut self, id: ProjectId, name: String) -> Result<(), CreateError> {
        if let Some(&owner) = self.by_name.get(&name) {
            return if owner == id {
                Ok(())
            } else {
                Err(CreateError::DuplicateName)
            };
        }
        let Some(p) = self.by_id.get_mut(&id) else {
            return Ok(()); // gone; nothing to rename
        };
        self.by_name.remove(&p.name);
        p.name = name.clone();
        self.by_name.insert(name, id);
        Ok(())
    }

    /// Repoint a project's cwd, keeping cwd-uniqueness. Refuses if another
    /// project already roots at the new cwd. **New** spawns/pickers/grouping
    /// follow immediately (everything resolves live); an already-running agent
    /// subprocess keeps its original spawn cwd (server-side, immutable) —
    /// ADR-0028 §Consequences.
    pub(crate) fn set_cwd(&mut self, id: ProjectId, cwd: PathBuf) -> Result<(), CreateError> {
        let key = cwd_match_key(&cwd);
        if let Some(&owner) = self.by_cwd.get(&key) {
            if owner != id {
                return Err(CreateError::DuplicateCwd(owner));
            }
            return Ok(()); // same cwd (canonically); no-op
        }
        let Some(p) = self.by_id.get_mut(&id) else {
            return Ok(());
        };
        self.by_cwd.remove(&cwd_match_key(&p.cwd));
        p.cwd = cwd;
        self.by_cwd.insert(key, id);
        Ok(())
    }

    pub(crate) fn by_name(&self, name: &str) -> Option<ProjectId> {
        self.by_name.get(name).copied()
    }

    /// Resolve a cwd to the project rooted there (canonical match; ADR-0010).
    /// **Total-or-none** thanks to cwd-uniqueness — never an ambiguous "first
    /// match." This is how a project-agnostic **roster/server session** infers
    /// its project ([`Membership::Inferred`]).
    pub(crate) fn by_cwd(&self, cwd: &Path) -> Option<ProjectId> {
        self.by_cwd.get(&cwd_match_key(cwd)).copied()
    }

    /// Classify a cwd into a [`Membership`] for a session/workspace that carries
    /// no stored assignment: `Inferred` if a project roots there, else
    /// `Unfiled`. (An `Assigned` membership is formed by the caller from a stored
    /// `ProjectId`, not here.)
    pub(crate) fn membership_for_cwd(&self, cwd: &Path) -> Membership {
        match self.by_cwd(cwd) {
            Some(id) => Membership::Inferred(id),
            None => Membership::Unfiled(cwd.to_path_buf()),
        }
    }

    pub(crate) fn get(&self, id: ProjectId) -> Option<&Project> {
        self.by_id.get(&id)
    }

    pub(crate) fn get_mut(&mut self, id: ProjectId) -> Option<&mut Project> {
        self.by_id.get_mut(&id)
    }

    /// The cwd of a project — the single source of truth every workspace/session
    /// reads through. `None` if the id is stale.
    pub(crate) fn cwd_of(&self, id: ProjectId) -> Option<&Path> {
        self.by_id.get(&id).map(|p| p.cwd.as_path())
    }

    /// The display name of a project, or `"?"` for a stale id (never panics in a
    /// render path).
    pub(crate) fn name_of(&self, id: ProjectId) -> &str {
        self.by_id.get(&id).map(|p| p.name.as_str()).unwrap_or("?")
    }

    /// Remove a project. Callers cascade its workspaces/sessions first
    /// (`UXI-Project-5`); this only drops the store entry + its reservations.
    pub(crate) fn close(&mut self, id: ProjectId) -> Option<Project> {
        let p = self.by_id.remove(&id)?;
        self.by_name.remove(&p.name);
        self.by_cwd.remove(&cwd_match_key(&p.cwd));
        Some(p)
    }

    pub(crate) fn contains(&self, id: ProjectId) -> bool {
        self.by_id.contains_key(&id)
    }

    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The first project by id, if any — the last-resort fallback for the
    /// "active project" derivation and self-heal (`UXI-Project-7`).
    pub(crate) fn first(&self) -> Option<ProjectId> {
        self.by_id.keys().next().copied()
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = ProjectId> + '_ {
        self.by_id.keys().copied()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (ProjectId, &Project)> + '_ {
        self.by_id.iter().map(|(id, p)| (*id, p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// UXI-Project-1 — name AND cwd are both unique keys; a second create with an
    /// existing name (or cwd) is refused and mutates nothing; `by_cwd` resolves.
    /// Negative control: drop the `by_name.contains_key` guard in `create` → the
    /// duplicate name is accepted and `len()==2`, so this fails.
    #[test]
    fn projects_store_enforces_unique_name() {
        let mut ps = Projects::new();
        let yalda = ps.create("Yaldabaoth".into(), p("/ws/yaldabaoth")).unwrap();
        // Same name, different cwd → refused, nothing changes.
        assert_eq!(
            ps.create("Yaldabaoth".into(), p("/ws/other")),
            Err(CreateError::DuplicateName)
        );
        // Same cwd, different name → refused with the owner.
        assert_eq!(
            ps.create("Other".into(), p("/ws/yaldabaoth")),
            Err(CreateError::DuplicateCwd(yalda))
        );
        assert_eq!(ps.len(), 1, "neither duplicate created a second project");
        assert_eq!(ps.cwd_of(yalda), Some(p("/ws/yaldabaoth").as_path()));

        // A distinct name AND cwd creates a distinct project.
        let fulcrum = ps.create("Fulcrum".into(), p("/ws/fulcrum")).unwrap();
        assert_ne!(yalda, fulcrum);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps.by_name("Fulcrum"), Some(fulcrum));
        assert_eq!(ps.by_cwd(&p("/ws/yaldabaoth")), Some(yalda));
        assert_eq!(ps.by_cwd(&p("/ws/nowhere")), None);
    }

    /// `ensure_at_cwd` returns the existing project for a known cwd and
    /// uniquifies the name for a new cwd whose basename-hint clashes — keeping
    /// BOTH invariants total (the migration's re-point path).
    #[test]
    fn ensure_at_cwd_dedups_cwd_and_uniquifies_name() {
        let mut ps = Projects::new();
        let a = ps.ensure_at_cwd(p("/one/archon"), "Archon");
        let a2 = ps.ensure_at_cwd(p("/one/archon"), "Archon");
        assert_eq!(a, a2, "same cwd folds to one project");
        assert_eq!(ps.len(), 1);
        // A DIFFERENT cwd with the same basename hint gets a uniquified name, not
        // a fold — both projects survive with distinct names + cwds.
        let b = ps.ensure_at_cwd(p("/two/archon"), "Archon");
        assert_ne!(a, b);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps.name_of(a), "Archon");
        assert_eq!(ps.name_of(b), "Archon (2)");
    }

    /// Membership classification: a cwd with a project infers it; a cwd without
    /// one is unfiled.
    #[test]
    fn membership_infers_or_unfiles_by_cwd() {
        let mut ps = Projects::new();
        let a = ps.create("A".into(), p("/a")).unwrap();
        assert_eq!(ps.membership_for_cwd(&p("/a")), Membership::Inferred(a));
        assert_eq!(ps.membership_for_cwd(&p("/a")).project(), Some(a));
        assert_eq!(
            ps.membership_for_cwd(&p("/b")),
            Membership::Unfiled(p("/b"))
        );
        assert_eq!(ps.membership_for_cwd(&p("/b")).project(), None);
    }

    /// `rename` / `set_cwd` keep their uniqueness invariants.
    #[test]
    fn rename_and_repoint_preserve_uniqueness() {
        let mut ps = Projects::new();
        let a = ps.create("A".into(), p("/a")).unwrap();
        let b = ps.create("B".into(), p("/b")).unwrap();

        // Rename onto a taken name is refused; same-name is a no-op.
        assert_eq!(ps.rename(a, "B".into()), Err(CreateError::DuplicateName));
        assert!(ps.rename(a, "A".into()).is_ok());
        assert!(ps.rename(a, "A2".into()).is_ok());
        assert_eq!(ps.by_name("A"), None);
        assert_eq!(ps.by_name("A2"), Some(a));

        // Repoint onto a taken cwd is refused; a fresh cwd moves the index.
        assert_eq!(ps.set_cwd(a, p("/b")), Err(CreateError::DuplicateCwd(b)));
        assert!(ps.set_cwd(a, p("/a2")).is_ok());
        assert_eq!(ps.by_cwd(&p("/a")), None, "old cwd released");
        assert_eq!(ps.by_cwd(&p("/a2")), Some(a), "new cwd routes");
    }

    /// `close` frees both the name and the cwd for reuse and never reuses ids.
    #[test]
    fn close_frees_name_and_cwd() {
        let mut ps = Projects::new();
        let a = ps.create("A".into(), p("/a")).unwrap();
        assert!(ps.contains(a));
        let removed = ps.close(a);
        assert_eq!(removed.map(|pr| pr.name), Some("A".to_string()));
        assert!(!ps.contains(a));
        assert_eq!(ps.by_name("A"), None);
        assert_eq!(ps.by_cwd(&p("/a")), None);
        // Name + cwd reusable; a fresh create mints a NEW id.
        let a2 = ps.create("A".into(), p("/a")).unwrap();
        assert_ne!(a, a2, "ids are never reused");
    }
}
