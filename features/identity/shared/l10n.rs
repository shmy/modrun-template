/// 域自注册 identity FTL；infra/locale 只保留 shared.ftl。
pub fn register_identity_locales() {
    locale::register_ftl("en-US", include_str!("../locales/en-US/identity.ftl"));
    locale::register_ftl("zh-CN", include_str!("../locales/zh-CN/identity.ftl"));
}
