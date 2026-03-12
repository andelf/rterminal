# Agent TUI 改进计划（架构与可维护性）

目标：在不破坏现有功能前提下，提升焦点/生命周期一致性、降低重复代码成本、补齐可回归测试。

## 阶段 0：质量门槛统一（已完成）
- [x] 将 `cargo clippy --all-targets -- -D warnings` 作为本地通过门槛。
- [x] 修复当前 clippy 阻塞项（dead_code、test 模块位置、可折叠 if、单字符 push_str）。
- 验证：`cargo check` + `cargo test` + `cargo clippy --all-targets -- -D warnings`。

## 阶段 1：Tab 焦点状态机收敛（已完成）
- [x] 统一 “激活 tab => 强制焦点落到 active terminal”。
- [x] 修复点击“当前 tab”不重新聚焦的问题。
- [x] 修复 shell-exit 触发关闭 tab 后焦点未一致恢复的问题。
- [x] 提取纯函数并补测试，覆盖 close 后 active index 变化规则。
- 验证：阶段 0 全量 + 新增单元测试。

## 阶段 2：重复代码收敛（已完成）
- [x] 收敛 tab 索引切换处理的重复逻辑（数据驱动/宏化，保持行为一致）。
- [x] 收敛输入模块中明显重复包装函数（保持 API 兼容）。
- 验证：阶段 0 全量 + 行为回归测试。

## 阶段 3：Debug Server 生命周期治理（待开始）
- [ ] 设计并实现单实例 debug server + 会话路由（或同等泄漏治理方案）。
- [ ] 明确 tab 创建/销毁与 debug 会话注册/注销关系。
- [ ] 覆盖多 tab 开关后的端口与可用性回归测试。
- 验证：阶段 0 全量 + 新增 debug 生命周期测试。

## 里程碑验收标准
1. 不引入功能回退（现有自动化测试持续通过）。
2. 每阶段结束至少完成一次全量验证并记录结果。
3. 对每个线上问题修复，至少增加一个能复现/防回归的测试点。

## 当前验证记录
- 2026-03-13：阶段 0 + 阶段 1 + 阶段 2（部分）完成后，`cargo check` / `cargo test`（15 tests）/ `cargo clippy --all-targets -- -D warnings` 全部通过。
- 2026-03-13：阶段 2 全量完成后，再次通过 `cargo check` / `cargo test`（15 tests）/ `cargo clippy --all-targets -- -D warnings` / `cargo run -- --self-check`。
