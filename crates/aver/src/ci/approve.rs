//! `aver ci approve` (ARCHITECTURE.md §2.9, approval): a person's pull request
//! approval, bound to the head commit it was given on, as what clears a
//! `REVIEW` or `UNPROVEN` check in CI. No check runs here.
//!
//! GitHub's reviews are the evidence: an approval binds only when it is the
//! reviewer's latest decision, was given on the pull request's current head
//! commit, and came from a human who is not the pull request's author. A push
//! after the review moves the head, so the approval no longer binds.

use std::collections::BTreeMap;

use aver_core::GitSha;
use serde::{Deserialize, Serialize};

/// The `pull_request_review` event fields `aver ci approve` reads.
#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub pull_request: EventPullRequest,
    pub review: Option<EventReview>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventPullRequest {
    pub number: u64,
    pub user: User,
    pub base: EventBase,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventBase {
    pub repo: Repository,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    pub full_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventReview {
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct User {
    pub login: String,
    #[serde(rename = "type")]
    pub kind: String,
}

impl User {
    /// A person's account: GitHub's type `User` and no `[bot]` login. Apps
    /// and the Actions bot approve as `Bot`.
    pub fn is_human(&self) -> bool {
        self.kind == "User" && !self.login.ends_with("[bot]")
    }
}

/// The pull request as `GET /repos/{repo}/pulls/{n}` returns it now.
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequest {
    pub head: Head,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Head {
    pub sha: GitSha,
}

/// One entry of `GET /repos/{repo}/pulls/{n}/reviews`, oldest first.
#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    pub id: u64,
    pub state: String,
    /// The head commit the review was given on; absent on very old ones.
    pub commit_id: Option<String>,
    /// `null` for a deleted account.
    pub user: Option<User>,
    /// The reviewer's relation to the repository: `OWNER`, `MEMBER`,
    /// `COLLABORATOR`, `CONTRIBUTOR`, `NONE`, …
    #[serde(default)]
    pub author_association: Option<String>,
}

/// The associations of reviewers who can push to the repository. Anyone
/// can leave an approving review on a public repository; only these count,
/// as for GitHub's own required reviews.
const MAINTAINERS: &[&str] = &["OWNER", "MEMBER", "COLLABORATOR"];

/// Why no approval binds to the head commit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("no approval of the pull request's current head {head}")]
    None { head: GitSha },
    #[error("review {review} by {login} approved {commit}, not the current head {head}: a push after the review needs a new approval")]
    Stale {
        review: u64,
        login: String,
        commit: String,
        head: GitSha,
    },
    #[error("review {review} is by {login}, the pull request's author, who cannot approve their own change")]
    Author { review: u64, login: String },
    #[error("review {review} is by {login}, which is not a person's account")]
    Bot { review: u64, login: String },
    #[error(
        "review {review} is by {login}, who has no write access to the repository ({association})"
    )]
    Outsider {
        review: u64,
        login: String,
        association: String,
    },
}

/// The approval `aver ci approve` writes to `aver-out/approval.json`.
/// Unsigned: the CI key is throwaway, so a signature would add nothing to
/// what the review API already says; the record names the review, which
/// anyone can look up, and the tree the approval covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Approval {
    pub repository: String,
    pub pull_request: u64,
    pub reviewer: String,
    pub review_id: u64,
    pub head_commit: GitSha,
    pub head_tree: GitSha,
    /// The `REVIEW` or `UNPROVEN` receipt the reviewer read, when known.
    pub receipt: Option<String>,
}

/// The review whose approval binds to `head`, preferring the one that
/// triggered the event, whose refusal is the one reported when none binds. Each reviewer's latest decision counts: a later
/// `CHANGES_REQUESTED` or a dismissal withdraws an approval.
pub fn binding<'a>(
    event: &Event,
    head: &GitSha,
    reviews: &'a [Review],
) -> Result<&'a Review, Refusal> {
    let mut latest: BTreeMap<&str, &Review> = BTreeMap::new();
    for review in reviews {
        let Some(user) = &review.user else { continue };
        if matches!(
            review.state.as_str(),
            "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"
        ) {
            latest.insert(user.login.as_str(), review);
        }
    }
    let mut approvals: Vec<&Review> = latest
        .into_values()
        .filter(|r| r.state == "APPROVED")
        .collect();
    approvals.sort_by_key(|r| r.id);
    let trigger = event.review.as_ref().map(|r| r.id);
    if let Some(i) = approvals.iter().position(|r| Some(r.id) == trigger) {
        let r = approvals.remove(i);
        approvals.insert(0, r);
    }
    let mut refusal = None;
    for review in approvals {
        match check(event, head, review) {
            Ok(()) => return Ok(review),
            Err(why) => {
                refusal.get_or_insert(why);
            }
        }
    }
    Err(refusal.unwrap_or(Refusal::None { head: head.clone() }))
}

fn check(event: &Event, head: &GitSha, review: &Review) -> Result<(), Refusal> {
    let user = review
        .user
        .as_ref()
        .expect("reviews without a user are skipped");
    if !user.is_human() {
        return Err(Refusal::Bot {
            review: review.id,
            login: user.login.clone(),
        });
    }
    if user.login == event.pull_request.user.login {
        return Err(Refusal::Author {
            review: review.id,
            login: user.login.clone(),
        });
    }
    let association = review.author_association.as_deref().unwrap_or("NONE");
    if !MAINTAINERS.contains(&association) {
        return Err(Refusal::Outsider {
            review: review.id,
            login: user.login.clone(),
            association: association.to_owned(),
        });
    }
    let commit = review.commit_id.as_deref().unwrap_or("");
    if commit != head.as_str() {
        return Err(Refusal::Stale {
            review: review.id,
            login: user.login.clone(),
            commit: if commit.is_empty() {
                "no commit".to_owned()
            } else {
                commit.to_owned()
            },
            head: head.clone(),
        });
    }
    Ok(())
}

/// Every review in `gh api --paginate`'s output: one JSON array per page,
/// written one after another.
pub fn parse_reviews(pages: &[u8]) -> Result<Vec<Review>, serde_json::Error> {
    let mut out = Vec::new();
    for page in serde_json::Deserializer::from_slice(pages).into_iter::<Vec<Review>>() {
        out.extend(page?);
    }
    Ok(out)
}

impl Event {
    /// The repository the pull request targets, `owner/name`.
    pub fn repository(&self) -> &str {
        &self.pull_request.base.repo.full_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "2222222222222222222222222222222222222222";
    const OLD: &str = "1111111111111111111111111111111111111111";

    fn event(trigger: u64) -> Event {
        serde_json::from_value(serde_json::json!({
            "pull_request": {"number": 7, "user": {"login": "author", "type": "User"},
                "base": {"repo": {"full_name": "o/r"}}},
            "review": {"id": trigger}
        }))
        .unwrap()
    }

    fn review(id: u64, state: &str, commit: &str, login: &str, kind: &str) -> Review {
        serde_json::from_value(serde_json::json!({
            "id": id, "state": state, "commit_id": commit,
            "user": {"login": login, "type": kind}, "author_association": "MEMBER"
        }))
        .unwrap()
    }

    fn head() -> GitSha {
        HEAD.parse().unwrap()
    }

    #[test]
    fn a_human_approval_on_the_head_binds() {
        let reviews = [review(1, "APPROVED", HEAD, "alice", "User")];
        assert_eq!(binding(&event(1), &head(), &reviews).unwrap().id, 1);
    }

    #[test]
    fn an_approval_of_an_older_commit_is_stale() {
        let reviews = [review(1, "APPROVED", OLD, "alice", "User")];
        assert!(matches!(
            binding(&event(1), &head(), &reviews),
            Err(Refusal::Stale { review: 1, .. })
        ));
    }

    #[test]
    fn the_authors_and_bots_approvals_never_bind() {
        let reviews = [review(1, "APPROVED", HEAD, "author", "User")];
        assert!(matches!(
            binding(&event(1), &head(), &reviews),
            Err(Refusal::Author { .. })
        ));
        for (login, kind) in [("ci[bot]", "User"), ("app", "Bot")] {
            let reviews = [review(2, "APPROVED", HEAD, login, kind)];
            assert!(matches!(
                binding(&event(2), &head(), &reviews),
                Err(Refusal::Bot { .. })
            ));
        }
    }

    #[test]
    fn an_approval_from_outside_the_repository_never_binds() {
        for association in [None, Some("CONTRIBUTOR"), Some("NONE")] {
            let mut r = review(1, "APPROVED", HEAD, "mallory", "User");
            r.author_association = association.map(str::to_owned);
            assert!(matches!(
                binding(&event(1), &head(), &[r]),
                Err(Refusal::Outsider { .. })
            ));
        }
    }

    #[test]
    fn a_later_change_request_withdraws_an_approval() {
        let reviews = [
            review(1, "APPROVED", HEAD, "alice", "User"),
            review(2, "COMMENTED", HEAD, "alice", "User"),
            review(3, "CHANGES_REQUESTED", HEAD, "alice", "User"),
        ];
        assert_eq!(
            binding(&event(3), &head(), &reviews).unwrap_err(),
            Refusal::None { head: head() }
        );
    }

    #[test]
    fn another_reviewers_approval_on_the_head_binds() {
        let reviews = [
            review(1, "APPROVED", HEAD, "bob", "User"),
            review(2, "APPROVED", OLD, "alice", "User"),
        ];
        assert_eq!(binding(&event(2), &head(), &reviews).unwrap().id, 1);
    }

    #[test]
    fn paginated_pages_are_read_one_after_another() {
        let pages = br#"[{"id":1,"state":"APPROVED","commit_id":null,"user":null}]
[{"id":2,"state":"COMMENTED","commit_id":"x","user":{"login":"a","type":"User"}}]"#;
        let ids: Vec<u64> = parse_reviews(pages).unwrap().iter().map(|r| r.id).collect();
        assert_eq!(ids, [1, 2]);
    }
}
