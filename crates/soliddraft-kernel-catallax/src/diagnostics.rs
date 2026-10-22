//! Opt-in observations of the kernel calls the application actually executes.

use std::{cell::RefCell, sync::Mutex, time::Instant};

use catallax_numbers::{BoundKind, MarkLog, OperationEntry, with_operation_budget, with_operation_log};
use serde_json::{Value, json};
use soliddraft_kernel_api::{KernelError, KernelResult};

thread_local! {
    static ACTIVE: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct Scope;

impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| *active.borrow_mut() = None);
    }
}

pub(crate) struct Diagnostics {
    max_work: u64,
    calls: Mutex<Vec<Value>>,
}

impl Diagnostics {
    pub(crate) fn new(max_work: u64) -> Self {
        Self { max_work, calls: Mutex::new(Vec::new()) }
    }

    pub(crate) fn take(&self) -> Vec<Value> {
        std::mem::take(&mut *self.calls.lock().expect("diagnostics lock"))
    }

    pub(crate) fn observe<T>(&self, operation: &str, run: impl FnOnce() -> KernelResult<T>) -> KernelResult<T> {
        let start = Instant::now();
        if let Some(parent) = ACTIVE.with(|active| active.borrow().clone()) {
            let result = run();
            self.calls.lock().expect("diagnostics lock").push(json!({
                "operation":operation,"log_scope":parent,"nested":true,
                "status":if result.is_ok() {"answered"} else {"refused"},
                "error":result.as_ref().err().map(ToString::to_string),
                "max_work":null,"work":null,"wall_ms":start.elapsed().as_secs_f64()*1000.0,
                "operation_log":{"format":"catallax-operation-log/1","operations":[]}
            }));
            return result;
        }
        ACTIVE.with(|active| *active.borrow_mut() = Some(operation.to_owned()));
        let _scope = Scope;
        let ((mut result, work), entries) = with_operation_log(|| with_operation_budget(self.max_work, run));
        if work.exhausted {
            result =
                Err(KernelError::Unavailable("numbers.operation-budget: operation work allowance exhausted".into()));
        }
        self.calls.lock().expect("diagnostics lock").push(json!({
            "operation":operation, "log_scope":operation,"nested":false, "status":if result.is_ok() {"answered"} else {"refused"},
            "error":result.as_ref().err().map(ToString::to_string),
            "max_work":self.max_work, "work":{"used":work.used,"exhausted":work.exhausted},
            "wall_ms":start.elapsed().as_secs_f64()*1000.0,
            "operation_log":{"format":"catallax-operation-log/1", "operations":entries.iter().enumerate()
                .map(|(sequence, e)| entry(sequence, e)).collect::<Vec<_>>()}
        }));
        result
    }
}

fn marks(log: &MarkLog, step: &str) -> Vec<Value> {
    log.iter()
        .map(|m| {
            json!({"code":m.code,"detail":m.detail,"step":step,
            "bound":m.bound.to_decimal_bounds(40).1, "kind":match m.kind {
                BoundKind::Distance=>"distance",BoundKind::Angle=>"angle",BoundKind::Value=>"value"
            }})
        })
        .collect()
}

fn entry(sequence: usize, e: &OperationEntry) -> Value {
    let decisions = e.after.as_ref().map(|a| {
        let b = &e.before;
        json!({"total":a.decisions.saturating_sub(b.decisions),
            "filter":a.filter.saturating_sub(b.filter), "refined":a.refined.saturating_sub(b.refined),
            "exact":a.exact.saturating_sub(b.exact),
            "coincident_exact":a.coincident_exact.saturating_sub(b.coincident_exact),
            "coincident_separation":a.coincident_separation.saturating_sub(b.coincident_separation),
            "coincident_fact":a.coincident_fact.saturating_sub(b.coincident_fact),
            "not_exact":a.not_exact.saturating_sub(b.not_exact), "refused":a.refused.saturating_sub(b.refused),
            "max_bits_high_water":a.max_bits})
    });
    json!({"sequence":sequence,"kernel":e.kernel,"id":e.id,"operation":e.operation,
        "status":if e.after.is_none() {"interrupted"} else if e.refusal.is_some() {"refused"} else {"answered"},
        "decisions":decisions,"marks":marks(&e.marks,&e.id),"local_tolerances":null,
        "refusal":e.refusal.as_ref().map(|r| json!({"code":r.code,"detail":r.detail})),
        "work":{"before":e.work_before,"after":e.work_after,"used":e.work_after-e.work_before},"wall_ms":e.wall_ms})
}

#[cfg(test)]
mod tests {
    use super::*;
    use catallax_numbers::{Decider, OperationSpan};

    #[test]
    fn nested_queries_share_one_log_and_work_scope() {
        let diagnostics = Diagnostics::new(100);
        diagnostics
            .observe("view_curves", || {
                diagnostics.observe("tessellate", || {
                    let decider = Decider::default();
                    OperationSpan::begin("catallax", &"mesh", "mesh", &decider).finish(&decider, None, None);
                    Ok(())
                })
            })
            .unwrap();
        let calls = diagnostics.take();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["operation"], "tessellate");
        assert_eq!(calls[0]["log_scope"], "view_curves");
        assert!(calls[0]["work"].is_null());
        assert_eq!(calls[1]["operation_log"]["operations"][0]["operation"], "mesh");
        diagnostics.observe("next_request", || Ok(())).unwrap();
        assert_eq!(diagnostics.take()[0]["nested"], false);
    }
}
