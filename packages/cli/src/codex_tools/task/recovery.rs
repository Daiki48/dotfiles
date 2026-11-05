//! 技術的な停止を読み取り専用の診断へ戻し、変化のない外部待機にAIを起動しない。
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Phase, Result, Task, now, safe_text};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Boundary {
    Pr,
    Merge,
    #[default]
    Finish,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Decision {
    pub kind: String,
    pub question: String,
    pub impact: String,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Followup {
    pub objective: String,
    pub authorization: String,
    pub after_previous: bool,
    #[serde(default)]
    pub purpose: FollowupPurpose,
    #[serde(default)]
    pub reuse_task_id: Option<String>,
    #[serde(default)]
    pub distinct_from: Vec<FollowupDistinction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FollowupDistinction {
    pub task_id: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FollowupPurpose {
    #[default]
    Continuation,
    RepairMainCi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Assessment {
    pub cause: String,
    pub evidence: String,
    pub next_action: String,
    pub resume_when: String,
    pub decision: Option<Decision>,
    pub watch_paths: Vec<String>,
    pub followups: Vec<Followup>,
}

impl Assessment {
    pub fn validate(&self) -> Result<()> {
        if [
            &self.cause,
            &self.evidence,
            &self.next_action,
            &self.resume_when,
        ]
        .iter()
        .any(|text| !safe_text(text, 4096))
            || self.watch_paths.len() > 8
            || self.watch_paths.iter().any(|path| !safe_relative(path))
            || self.followups.len() > 4
            || self.followups.iter().any(|next| {
                !safe_text(&next.objective, 8192)
                    || !safe_text(&next.authorization, 4096)
                    || next
                        .reuse_task_id
                        .as_deref()
                        .is_some_and(|id| !super::valid_id(id))
                    || next.distinct_from.len() > 32
                    || next.distinct_from.iter().any(|entry| {
                        !super::valid_id(&entry.task_id) || !safe_text(&entry.reason, 2048)
                    })
                    || next.reuse_task_id.is_some() && !next.distinct_from.is_empty()
            })
            || self.decision.as_ref().is_some_and(|decision| {
                ![
                    "product",
                    "scope",
                    "permission",
                    "cost",
                    "irreversible",
                    "residual-risk",
                ]
                .contains(&decision.kind.as_str())
                    || [
                        &decision.question,
                        &decision.impact,
                        &decision.recommendation,
                    ]
                    .iter()
                    .any(|text| !safe_text(text, 4096))
            })
        {
            return Err("診断の根拠・次の対応・意思決定・監視対象が不正です".into());
        }
        Ok(())
    }
}

pub(super) fn safe_relative(path: &str) -> bool {
    use std::path::{Component, Path};
    safe_text(path, 512)
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(name) if name != ".git"))
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct State {
    pub origin: Option<Phase>,
    pub assessment: Option<Assessment>,
    pub attempts: u32,
    pub evidence: String,
    pub progress_observation: String,
    pub observation: String,
    pub next_observation: u64,
    pub children: Vec<String>,
    pub parent: Option<String>,
    pub repairs_main: bool,
    pub repair_task: Option<String>,
    pub next_authorization: u64,
    pub authorization_attempts: u32,
    pub barrier: bool,
    pub last_scheduled: u64,
    pub repair_evidence: Option<RepairEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RepairEvidence {
    pub task_id: String,
    #[serde(default)]
    pub verified_task_id: String,
    pub original_merge: String,
    pub observation: super::ci::Observation,
}

impl Task {
    pub(super) fn diagnose(&mut self, reason: &str) {
        if !matches!(self.phase, Phase::Diagnosis | Phase::Waiting) {
            self.recovery.origin = Some(
                if matches!(self.phase, Phase::Blocked | Phase::AwaitingDecision) {
                    self.resume_phase.clone()
                } else {
                    self.phase.clone()
                },
            );
        }
        if self.merge_commit.is_some()
            || matches!(
                self.recovery.origin,
                Some(Phase::Integration | Phase::MainCi | Phase::Finish)
            )
        {
            self.recovery.barrier = true;
        }
        self.phase = Phase::Diagnosis;
        self.checkpoint = reason.into();
        self.recovery.next_observation = 0;
        self.event("diagnosis", reason);
    }

    pub(super) fn monitor(&mut self, reason: &str) {
        self.phase = Phase::Waiting;
        self.checkpoint = reason.into();
        self.recovery.next_observation = now() + 120;
        self.event("monitoring", reason);
    }

    pub(super) fn effective_phase(&self) -> &Phase {
        if matches!(self.phase, Phase::Diagnosis | Phase::Waiting) {
            self.recovery.origin.as_ref().unwrap_or(&self.resume_phase)
        } else {
            &self.phase
        }
    }

    pub(super) fn record_assessment(&mut self, assessment: &Assessment) {
        let evidence = format!(
            "{:x}",
            Sha256::digest(format!("{}\n{}", assessment.cause, assessment.evidence))
        );
        let observed = format!(
            "{:?}|{}|{:?}",
            self.head, self.ci_fingerprint, self.ci_state
        );
        if self.recovery.evidence.is_empty()
            || self.recovery.evidence != evidence && self.recovery.progress_observation != observed
        {
            self.recovery.attempts = 0;
        }
        self.recovery.evidence = evidence;
        self.recovery.progress_observation = observed;
        self.recovery.assessment = Some(assessment.clone());
    }
}

/// ageを加算するため、新しい実装taskが増え続けてもreview/後処理が飢餓化しない。
pub(super) fn scheduling_score(task: &Task, at: u64) -> u64 {
    let waiting_since = if task.recovery.last_scheduled == 0 {
        task.created
    } else {
        task.recovery.last_scheduled
    };
    let bias = match task.phase {
        Phase::Implementation => 60,
        Phase::Review => 90,
        Phase::Integration | Phase::Finish => 120,
        Phase::Diagnosis => 30,
        _ => 0,
    };
    at.saturating_sub(waiting_since).saturating_add(bias)
}
