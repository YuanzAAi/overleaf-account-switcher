use std::collections::BTreeSet;

use overleaf_core::{Project, ProjectAccessLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMigrationStrategy {
    OwnedProject,
    LinkSharingCollaboration,
    CopyCollaboration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMigrationPlan {
    pub project_id: String,
    pub project_name: String,
    pub strategy: ProjectMigrationStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiAttemptResult {
    Success,
    RateLimited,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinAttemptDecision {
    Joined,
    RetryAfterSeconds(u64),
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectMigrationEvidence {
    pub target_projects: Vec<Project>,
    pub original_accessible: BTreeSet<String>,
    pub cloned_project_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMigrationAudit {
    pub confirmed: Vec<ProjectMigrationPlan>,
    pub missing: Vec<ProjectMigrationPlan>,
}

pub fn rate_limit_wait_seconds(retry_index: u32) -> u64 {
    15 * (u64::from(retry_index) + 1)
}

pub fn plan_project_migrations(projects: &[Project]) -> Vec<ProjectMigrationPlan> {
    let mut active_projects = projects
        .iter()
        .filter(|project| !project.trashed)
        .collect::<Vec<_>>();
    // Overleaf 默认把最新创建/更新的项目排在前面。按源项目更新时间从旧到新
    // 逐个创建副本，才能让目标账号最终保持与源账号相同的展示顺序。
    active_projects.sort_by(|left, right| {
        left.last_updated
            .as_deref()
            .unwrap_or("")
            .cmp(right.last_updated.as_deref().unwrap_or(""))
    });
    active_projects
        .into_iter()
        .map(|project| ProjectMigrationPlan {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            strategy: migration_strategy_for_project(project),
        })
        .collect()
}

pub fn migration_strategy_for_project(project: &Project) -> ProjectMigrationStrategy {
    if project.is_owner() {
        ProjectMigrationStrategy::OwnedProject
    } else if project.is_link_sharing_collaboration() {
        ProjectMigrationStrategy::LinkSharingCollaboration
    } else {
        ProjectMigrationStrategy::CopyCollaboration
    }
}

pub fn resolve_join_attempt(
    project_id: &str,
    result: ApiAttemptResult,
    target_projects: &[Project],
    retry_index: u32,
) -> JoinAttemptDecision {
    match result {
        ApiAttemptResult::Success => JoinAttemptDecision::Joined,
        ApiAttemptResult::Failed => JoinAttemptDecision::Failed,
        ApiAttemptResult::RateLimited if target_can_access_project(target_projects, project_id) => {
            JoinAttemptDecision::Joined
        }
        ApiAttemptResult::RateLimited => {
            JoinAttemptDecision::RetryAfterSeconds(rate_limit_wait_seconds(retry_index))
        }
    }
}

pub fn audit_project_migrations(
    plans: &[ProjectMigrationPlan],
    evidence: &ProjectMigrationEvidence,
) -> ProjectMigrationAudit {
    let mut confirmed = Vec::new();
    let mut missing = Vec::new();

    for plan in plans {
        if is_migration_confirmed(plan, evidence) {
            confirmed.push(plan.clone());
        } else {
            missing.push(plan.clone());
        }
    }

    ProjectMigrationAudit { confirmed, missing }
}

pub fn plan_remigration_once(
    plans: &[ProjectMigrationPlan],
    evidence: &ProjectMigrationEvidence,
) -> Vec<ProjectMigrationPlan> {
    audit_project_migrations(plans, evidence).missing
}

pub fn is_migration_confirmed(
    plan: &ProjectMigrationPlan,
    evidence: &ProjectMigrationEvidence,
) -> bool {
    match plan.strategy {
        ProjectMigrationStrategy::OwnedProject => {
            target_has_owned_project_named(&evidence.target_projects, &plan.project_name)
                && !target_can_access_project(&evidence.target_projects, &plan.project_id)
        }
        ProjectMigrationStrategy::LinkSharingCollaboration => {
            target_can_access_project(&evidence.target_projects, &plan.project_id)
        }
        ProjectMigrationStrategy::CopyCollaboration => {
            target_can_access_project(&evidence.target_projects, &plan.project_id)
                || target_has_owned_project_named(&evidence.target_projects, &plan.project_name)
        }
    }
}

fn target_can_access_project(projects: &[Project], project_id: &str) -> bool {
    projects
        .iter()
        .any(|project| !project.trashed && project.id == project_id)
}

fn target_has_owned_project_named(projects: &[Project], project_name: &str) -> bool {
    projects.iter().any(|project| {
        !project.trashed
            && project.name == project_name
            && project.access_level == ProjectAccessLevel::Owner
    })
}
