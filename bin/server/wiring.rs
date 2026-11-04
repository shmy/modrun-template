use crate::internal_jobs;
use modrun::Module;

pub fn module() -> Module {
    Module::builder("server.wiring")
        .provide_group(internal_jobs::job_handlers)
        // 附件引用来源由各业务域 provide_group 贡献；组合根只把它交给 AttachmentGc。
        .provide_group(internal_jobs::attachment_job_handlers)
        .provide_group(internal_jobs::sched_jobs)
}
