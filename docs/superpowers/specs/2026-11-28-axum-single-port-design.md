# axum + tokio 单端口迁移设计

日期：2026-11-28
状态：已批准

## 背景与目标

rterminal 目前有两个网络监听：

- HTTP API：`tiny_http` 绑定 `127.0.0.1:7878`（`src/api_server.rs`），承载 `/web/*`、`/tabs/*`、`/debug/*`。
- WebSocket 数据流：裸 `TcpListener` + `tungstenite` 绑定 `127.0.0.1:7879`（`src/web_socket.rs`），路径 `/ws/tabs/<id>`。浏览器通过 `GET /web/config` 的 `ws_port` 字段发现端口。

`tiny_http` 上游自 2022-10 起未再发版，维护停滞。迁移到 `axum + tokio`（生产级标准栈），同时把 HTTP 与 WebSocket 合并到同一端口，消除端口发现逻辑。

## 已确认的决策

1. **单端口 7878** 承载 HTTP + WebSocket。
2. **`--ws-addr` 保留参数**，传入时打印 deprecation warning，**不保持行为兼容**（参数被忽略，7879 不再监听）。
3. **整体迁移**：HTTP 与 WS 都迁到 axum；业务层（`parse_request_with_client`、auth、web_debug、web_ui）原样复用。
4. **保留集成测试**：真实 bind + 真实 HTTP 请求的模式不变，客户端换成 async 请求库。

## 架构

```
main.rs ── start_api_server(&cli.api_addr, api_tx)   [签名不变，仍同步]
              └─ std::thread "agent-web"
                   └─ tokio multi-thread runtime (enable_all)
                        └─ axum Router serve, bind 127.0.0.1:7878
```

- 运行时放独立线程，gpui 主循环与 tokio 互不干扰。`async_channel` 两端原生通用：axum handler 里 `.send(cmd).await` / `rx.recv().await`；gpui 侧 `tabs.rs` 的 `api_rx.recv().await` 消费逻辑**零改动**。
- WS 路由：`GET /ws/tabs/<id>` 用 `WebSocketUpgrade` extractor（`axum::extract::ws`），握手由 axum 完成；`tungstenite` 依赖删除。
- `/web/config` 保留，`ws_port` 字段返回 HTTP 端口本身（老客户端无害）；`set_ws_port`/`WS_PORT` 机制可简化为直接读绑定端口，最终以实现简洁为准。

## 组件变更

| 单元 | 变更 |
|---|---|
| `src/api_server.rs` | tiny_http 请求循环 → axum Router + handler。`parse_request_with_client`、`RouteOutcome` 三分支、`render_reply`、`cache_policy`、`X-Rterminal-Client` 头、input trace 钩子全部保留；认证改为 axum layer（`is_public_path` 复用），`/web/auth` 的 `Set-Cookie` 照旧 |
| `src/web_socket.rs` | 删 `TcpListener`/握手/`percent_decode`；`serve_client` 变成 WS handler，消息循环 `.await` 化。批量发送语义保持：首块立即发、backlog 合并（`bounded(1)` stream channel 不变） |
| `src/auth.rs` / `src/web_ui.rs` / `src/api_protocol.rs` / `src/web_debug.rs` | 不动 |
| `src/cli.rs` / `src/main.rs` | `--ws-addr` 标 deprecated + warn；删 `start_ws_server` 调用、`warn_if_public_bind("WebSocket", ...)`、`set_ws_port` 时序（端口改为由 axum 绑定结果得出） |

## 依赖变化

- 新增：`axum`（`ws` feature）、`tokio`（`rt-multi-thread`、`net`、`macros`）
- 删除：`tiny_http`、`tungstenite`
- dev-dependencies 新增：async HTTP 客户端（集成测试用）、WebSocket 客户端（`tokio-tungstenite`，WS 集成测试用）
- tokio 已在依赖树中（gpui 传递引入），实际增量小

## 错误处理

- 端口绑定失败：保持现状语义——进程打印错误并 exit(1)。
- 命令通道关闭：handler 返回 503/504（现文案照搬）。
- WS 读错误/断开：`detach_stream` 清理逻辑保持。

## 测试计划

- 现有 4 个集成测试（`server_round_trip_get_tabs_via_fake_handler`、`server_returns_404_for_unknown_route`、`server_rejects_unauthenticated_api_requests`、`server_serves_immediate_help_for_debug_root`）保留真实 bind + 真实请求模式，客户端改为 async HTTP 请求，断言不变。
- 新增/改造 WS 集成测试：连接 `/ws/tabs/<id>?client=test`，验证 snapshot 首帧、resize 控制帧、input 二进制帧。
- `web_socket.rs` 的 `ephemeral_bind_returns_the_actual_port` 随旧入口删除，端口解析验证并入 axum 启动逻辑测试。
- 遵循 AGENTS.md：先编译（`cargo check`），再测试，才可声称完成。

## 非目标

- 不做 TLS 终止（维持明文 + loopback 默认；公网部署交给反代）。
- 不改 `api_protocol.rs` 命令集。
- 不动 gpui 侧终端渲染与 PTY 层。
