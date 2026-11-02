use utoipa_axum::router::OpenApiRouter;
use web_state::WebState;

pub type ApiRouter = OpenApiRouter<WebState>;

#[derive(Clone)]
pub struct PublicRouter(pub ApiRouter);

#[derive(Clone)]
pub struct ProtectedRouter(pub ApiRouter);

/// 健康探针路由：**不经过全局限流**。
///
/// K8s / LB 会以秒级频率探测 `/livez` `/readyz`；若与业务请求共用令牌桶，
/// 探针自身就会耗尽配额并被 429，liveness 进而误判为不健康触发重启风暴。
#[derive(Clone)]
pub struct ProbeRouter(pub ApiRouter);
