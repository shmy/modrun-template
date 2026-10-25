# File 域 HTTP E2E（Hurl）
#
# 依赖：种子管理员；对象存储可用（blob-fs 或 COS 配置正确）。
# 小 PNG 见 e2e/fixtures/one-by-one.png（相对本文件路径）。
#
#   just e2e_file
#   hurl --test --variables-file e2e/env e2e/file.hurl

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
file_admin_at: jsonpath "$.access_token"
[Asserts]
jsonpath "$.token_type" == "Bearer"

###############################################################################
# 上传：无鉴权
POST {{base_url}}/api/v1/files/images
[Multipart]
image: file,fixtures/one-by-one.png; image/png

HTTP 401
[Asserts]
jsonpath "$.error_code" == "access_token_missing"

###############################################################################
# 上传：非图片内容（扩展名 .png 误导检测）
POST {{base_url}}/api/v1/files/images
Authorization: Bearer {{file_admin_at}}
[Multipart]
image: file,fixtures/not-a-real.png; image/png

HTTP 400
[Asserts]
jsonpath "$.error_code" == "file_not_image"

###############################################################################
# 上传：合法 PNG → staging 路径 + 公网 URL
POST {{base_url}}/api/v1/files/images
Authorization: Bearer {{file_admin_at}}
[Multipart]
image: file,fixtures/one-by-one.png; image/png

HTTP 200
[Asserts]
jsonpath "$.path" matches /^staging\//
jsonpath "$.url" matches /^.+$/
