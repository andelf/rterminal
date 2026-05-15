# Agent TUI 改进计划（架构与可维护性）

## 已完成

- **阶段 0**：clippy 质量门槛统一
- **阶段 1**：Tab 焦点状态机收敛
- **阶段 2**：重复代码收敛、模块拆分（pty/input/render/debug_server/color/keyboard 等）

最后验证记录（2026-03-13）：`cargo check` / `cargo test`（15 tests）/ `cargo clippy --all-targets -- -D warnings` 全部通过。

## 待做

### 阶段 3：Debug Server 生命周期治理

- [ ] 设计并实现单实例 debug server + 会话路由（或同等泄漏治理方案）
- [ ] 明确 tab 创建/销毁与 debug 会话注册/注销关系
- [ ] 覆盖多 tab 开关后的端口与可用性回归测试
- [ ] 默认不启动 debug server，需 `--debug-http` 显式开启（安全加固）
