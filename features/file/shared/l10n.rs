/// 域自注册 file FTL；infra/locale 只保留 shared.ftl。
pub fn register_file_locales() {
    locale::register_ftl("en-US", include_str!("../locales/en-US/file.ftl"));
    locale::register_ftl("zh-CN", include_str!("../locales/zh-CN/file.ftl"));
}
