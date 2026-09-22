//! 分支与撤销：任何裁定都可通过“在旧事件上开新分支”撤销并恢复前一分支。
//! 分支是事件追加结构，不删除旧分支、人工裁定及其引用的原始字节。

use crate::db::Store;
use crate::events::EventKind;
use serde_json::json;

impl Store {
    pub fn fork_at(&self, new_branch: &str, label: &str, parent: &str, at_event: Option<&str>) -> Result<(), String> {
        if self.list_branches().iter().any(|b| b.branch_id == new_branch) {
            return Err("branch already exists".into());
        }
        self.create_branch(new_branch, label, parent, at_event);
        let refs = self.refs_at(parent, at_event);
        let event_id = at_event.map(String::from).or_else(|| self.tip(parent));
        self.append_event(
            new_branch,
            EventKind::BranchCreated,
            json!({"forked_from": parent, "at_event": event_id, "label": label}),
            &refs,
            json!({"note": "fork restores pinned dependency versions and visible conclusions"}),
            None,
        )
        .map_err(|e| e.message)?;
        Ok(())
    }

    /// 在同一分支上记录“撤销”事件：refs 回到目标旧事件的版本，旧事件仍保留可读。
    pub fn revert_to(&self, branch: &str, target_event: &str, expected_parent: Option<&str>) -> Result<String, String> {
        let target = self.event(target_event).ok_or("target event missing")?;
        let tip = self.tip(branch);
        let exp = expected_parent.map(String::from).or(tip);
        let ev = self
            .append_event(
                branch,
                EventKind::DecisionReverted,
                json!({"reverted_to": target_event}),
                &target.refs,
                json!({"note": "refs restored; superseded events remain readable, history not overwritten"}),
                exp.as_deref(),
            )
            .map_err(|e| e.message)?;
        // 撤销到的若是可评估 refs，重算一版新结论（不覆盖历史行）
        if target.refs.raw_events.is_some() {
            self.commit_appraisal(branch, &target.refs, &ev.event_id, "after-revert")?;
        }
        Ok(ev.event_id)
    }
}
