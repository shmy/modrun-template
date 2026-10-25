use job_queue_kit::JobBus;
use web_state::WebState;

/// 上下文能入队 Job（供 cron → enqueue 桥接）。
pub trait HasJobBus {
    fn job_bus(&self) -> &JobBus;
}

impl HasJobBus for WebState {
    fn job_bus(&self) -> &JobBus {
        &self.jobs
    }
}
