use utoipa_axum::router::OpenApiRouter;
use web_state::WebState;

pub type ApiRouter = OpenApiRouter<WebState>;

#[derive(Clone)]
pub struct PublicRouter(pub ApiRouter);

#[derive(Clone)]
pub struct ProtectedRouter(pub ApiRouter);
