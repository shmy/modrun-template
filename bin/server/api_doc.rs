use utoipa::openapi::security::SecurityScheme::Http as SecuritySchemeHttp;
use utoipa::{
    Modify,
    openapi::{
        OpenApi,
        security::{Http, HttpAuthScheme::Bearer},
    },
};

#[derive(utoipa::OpenApi)]
#[openapi(
    modifiers(&SecurityAddon),
    info(
        title = "Modrun Template API",
        description = "modrun-template HTTP API"
    ),
)]
pub(crate) struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme("bearerAuth", SecuritySchemeHttp(Http::new(Bearer)));
        }
    }
}
