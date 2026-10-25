# Identity E2E（Hurl）
#
# 前置：PostgreSQL 已启动、服务已运行（种子管理员见 .env.example）
#
#   just e2e
#   hurl --test --variables-file e2e/env e2e/identity.hurl

###############################################################################
# 管理员登录
POST {{base_url}}/api/v1/identity/login
Content-Type: application/json
```json
{
  "phone": "{{e2e_admin_phone}}",
  "password": "{{e2e_admin_password}}"
}
```

HTTP 200
[Captures]
admin_at_initial: jsonpath "$.access_token"
admin_rt_initial: jsonpath "$.refresh_token"
[Asserts]
jsonpath "$.token_type" == "Bearer"
jsonpath "$.access_token" matches /^.+$/
jsonpath "$.refresh_token" matches /^.+$/

###############################################################################
# 当前账号
GET {{base_url}}/api/v1/profile/current
Authorization: Bearer {{admin_at_initial}}

HTTP 200
[Asserts]
jsonpath "$.phone" == "{{e2e_admin_phone}}"
jsonpath "$.privileged" == true

###############################################################################
# 无令牌 → 401
GET {{base_url}}/api/v1/profile/current

HTTP 401
[Asserts]
jsonpath "$.error_code" == "access_token_missing"
jsonpath "$['type']" == "urn:modrun:problem:unauthorized"

###############################################################################
# 手机号过短
POST {{base_url}}/api/v1/identity/login
Accept-Language: en-US
Content-Type: application/json
```json
{
  "phone": "12",
  "password": "{{e2e_admin_password}}"
}
```

HTTP 400
[Asserts]
jsonpath "$.error_code" == "invalid_request_body"
jsonpath "$['type']" == "urn:modrun:problem:invalid-request-body"
jsonpath "$.detail" contains "Phone number is too short"

###############################################################################
# 错误密码
POST {{base_url}}/api/v1/identity/login
Content-Type: application/json
```json
{
  "phone": "{{e2e_admin_phone}}",
  "password": "wrong-password-for-hurl-9z"
}
```

HTTP 401
[Asserts]
header "Content-Type" contains "application/problem+json"
jsonpath "$.error_code" == "account_invalid_credentials"

###############################################################################
# refresh 旋转
POST {{base_url}}/api/v1/identity/refresh
Content-Type: application/json
```json
{
  "refresh_token": "{{admin_rt_initial}}"
}
```

HTTP 200
[Asserts]
jsonpath "$.token_type" == "Bearer"
jsonpath "$.access_token" != "{{admin_at_initial}}"
jsonpath "$.refresh_token" != "{{admin_rt_initial}}"
[Captures]
admin_at: jsonpath "$.access_token"
admin_rt: jsonpath "$.refresh_token"

###############################################################################
# 已消费的 refresh 再刷 → 失败
POST {{base_url}}/api/v1/identity/refresh
Content-Type: application/json
```json
{
  "refresh_token": "{{admin_rt_initial}}"
}
```

HTTP 401
[Asserts]
jsonpath "$.error_code" == "refresh_token_invalid"

###############################################################################
# 重放会吊销该账号全部会话（含 step6 新签发的 access）→ 重新登录取新会话，供后续用例使用
POST {{base_url}}/api/v1/identity/login
Content-Type: application/json
```json
{
  "phone": "{{e2e_admin_phone}}",
  "password": "{{e2e_admin_password}}"
}
```

HTTP 200
[Captures]
admin_at: jsonpath "$.access_token"
admin_rt: jsonpath "$.refresh_token"

###############################################################################
# 账号 CRUD（特权）
POST {{base_url}}/api/v1/accounts
Authorization: Bearer {{admin_at}}
Content-Type: application/json
```json
{
  "name": "e2e-user",
  "phone": "{{e2e_user_phone}}",
  "password": "e2euser12"
}
```

HTTP 200
[Captures]
e2e_user_id: jsonpath "$.id"
[Asserts]
jsonpath "$.name" == "e2e-user"

GET {{base_url}}/api/v1/accounts/{{e2e_user_id}}
Authorization: Bearer {{admin_at}}

HTTP 200
[Captures]
e2e_user_version: jsonpath "$.version"
[Asserts]
jsonpath "$.id" == "{{e2e_user_id}}"

PATCH {{base_url}}/api/v1/accounts/{{e2e_user_id}}
Authorization: Bearer {{admin_at}}
Content-Type: application/json
```json
{
  "name": "e2e-user",
  "phone": "{{e2e_user_phone}}",
  "version": 0
}
```

HTTP 409
[Asserts]
jsonpath "$.error_code" == "account_version_conflict"

PATCH {{base_url}}/api/v1/accounts/{{e2e_user_id}}/password
Authorization: Bearer {{admin_at}}
Content-Type: application/json
```json
{
  "new_password": "e2euser12"
}
```

HTTP 200

POST {{base_url}}/api/v1/identity/login
Content-Type: application/json
```json
{
  "phone": "{{e2e_user_phone}}",
  "password": "e2euser12"
}
```

HTTP 200
[Captures]
e2e_user_at: jsonpath "$.access_token"

GET {{base_url}}/api/v1/accounts
Authorization: Bearer {{e2e_user_at}}

HTTP 403
[Asserts]
jsonpath "$.error_code" == "account_forbidden"

DELETE {{base_url}}/api/v1/accounts/{{e2e_user_id}}
Authorization: Bearer {{admin_at}}

HTTP 200
[Asserts]
jsonpath "$.deleted" == true

GET {{base_url}}/api/v1/profile/current
Authorization: Bearer {{e2e_user_at}}

HTTP 401
[Asserts]
jsonpath "$.error_code" == "access_token_revoked"

###############################################################################
# 登出
POST {{base_url}}/api/v1/identity/logout
Authorization: Bearer {{admin_at}}

HTTP 200
[Asserts]
jsonpath "$.logged_out" == true

###############################################################################
# 登出后再访 profile → 401
GET {{base_url}}/api/v1/profile/current
Authorization: Bearer {{admin_at}}

HTTP 401
[Asserts]
jsonpath "$['type']" == "urn:modrun:problem:unauthorized"
jsonpath "$.error_code" == "access_token_revoked"
