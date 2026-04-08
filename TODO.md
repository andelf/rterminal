# agent-tui 代码审查改进计划

## 高优先级

### 1. 逐字符渲染 → 批量按行 shape
- **位置**: `src/main.rs:1304-1324`
- **问题**: 每帧对每个非空白字符单独调用 `shape_line()`，80×24 = 1920 次/帧
- **方案**: 按行批量 shape，一次绘制整行文本，减少 GPU 文本排版开销

### 2. 模块拆分
- **问题**: 单文件 2500+ 行，所有逻辑集中在 `src/main.rs`
- **方案**: 拆分为 `pty.rs`、`input.rs`、`render.rs`、`debug_server.rs`、`color.rs` 等模块

## 中优先级

### 3. AgentTerminal::new() 消除重复代码
- **位置**: `src/main.rs:547-659`
- **问题**: Ok/Err 分支各自构造实例，约 50 行重复
- **方案**: 提取公共构造逻辑，仅在 PTY 相关字段上分支

### 4. rewrite_terminal_input_line 宽字符光标定位
- **位置**: `src/main.rs:914-927`
- **问题**: 通过 N 次 `\x1b[D` 移动光标，中文等宽字符占两列会错位
- **方案**: 计算字符实际列宽（wcwidth），按列数发送左箭头

### 5. HTTP 调试接口安全加固
- **位置**: `src/main.rs:1496-1504`
- **问题**: 默认启动并监听 localhost:7878，`/debug/input` 可注入任意字节
- **方案**: 默认不启动，需 `--debug-http` 显式开启；或增加 token 认证

## 低优先级

### 6. indexed_to_rgb 传入正确 colors
- **位置**: `src/main.rs:1883-1913`
- **问题**: 索引 0-15 每次创建 `Default::default()` Colors，应使用当前终端 colors
- **方案**: 将 colors 参数传入 `indexed_to_rgb`

### 7. measure_cell_width 缓存
- **位置**: `src/main.rs:1414-1422` 及 render 闭包
- **问题**: 每帧重复测量字符宽度，值仅在字体/大小变化时才变
- **方案**: 缓存到 AgentTerminal 字段，仅在字体变化时重新测量

### 8. PTY reader 线程错误记录
- **位置**: `src/main.rs:1380-1393`
- **问题**: reader 线程 `Err(_)` 直接 break，不记录错误
- **方案**: eprintln 或通过 channel 通知主线程

## 功能缺失

### 9. Cmd+C 复制支持
- 已实现 Cmd+V 粘贴，但缺少选中文本复制到剪贴板

### 10. 滚动回看历史
- 缺少鼠标滚轮或 Shift+PageUp/PageDown 滚动

### 11. input_line 与 shell 状态同步
- **位置**: `src/main.rs:875-897`
- **问题**: `apply_terminal_bytes_to_input_line()` 未处理 Up/Down 历史导航、Tab 补全等场景
- **影响**: 使用方向键浏览历史后 input_line 与实际 shell 输入不一致
