use crate::internal_jobs;
use modrun::Module;

pub fn module() -> Module {
    Module::builder("server.wiring")
        .provide_group(internal_jobs::job_handlers)
        .provide_group(internal_jobs::sched_jobs)
}
