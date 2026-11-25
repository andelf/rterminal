# Agent TUI 改进计划（架构与可维护性）

## 已完成

- **阶段 0**：clippy 质量门槛统一
- **阶段 1**：Tab 焦点状态机收敛
- **阶段 2**：重复代码收敛、模块拆分（pty/input/render/api_server/color/keyboard 等）
- **阶段 3**：HTTP 服务生命周期与多 tab 路由
  - 单一全局 HTTP 服务，默认 `127.0.0.1:7878`
  - 所有 tab 使用稳定 id 寻址，创建/关闭不再启动或泄漏独立 debug server
  - 保留 `/debug/*` 兼容别名，新客户端使用 `/tabs/:id/*`
  - 最后一个 tab 关闭后保留空窗口和服务，可通过 API/Web 重新创建 tab

## 待做

### 阶段 4：HTTP 服务安全加固（可选）

当前 HTTP API 和 Web UI 是默认启用的核心功能，不再是临时 debug server。默认仅绑定 loopback，但没有身份认证。

- [ ] 评估是否提供 `--no-http` 或显式启用开关
- [ ] 如果允许非 loopback 绑定，增加认证或明确的启动警告
- [ ] 保持 `/tabs/:id/input`、WebSocket 输入和 `/debug/replace-line` 等写接口处于同一安全策略下
