# rterminal 待办改进清单

本清单只记录尚未完成、且已对照当前代码确认的问题。历史方案与调研结论见 `docs/` 和 `research/`。

## 高优先级

### 1. 逐字符渲染 → 批量按行 shape
- **位置**: `src/render.rs` canvas 闭包，每个非空白字符单独调用 `shape_line()`
- **问题**: 大终端网格每帧会产生大量文本排版调用
- **方案**: 按行拼接文本后批量 shape，一次绘制整行，减少 GPU 文本排版开销

## 中优先级

### 2. rewrite_terminal_input_line 宽字符光标定位
- **位置**: `src/input.rs` `rewrite_terminal_input_line()`
- **问题**: 通过 UTF-16 光标位置对应的次数发送 `\x1b[D`，中文等宽字符占两列时会错位
- **方案**: 计算光标后文本的实际终端列宽，按列数发送左箭头

### 3. input_line 与 shell 状态同步
- **位置**: `src/input.rs` `apply_terminal_bytes_to_input_line()`
- **问题**: Up/Down 历史导航、Tab 补全、tmux 等场景可能让 shadow input line 与实际 shell 状态不一致
- **方案**: 明确可追踪的输入子集；无法可靠同步时避免把 shadow model 当作 shell 状态的完整真值

### 4. PTY reader 线程错误静默
- **位置**: `src/pty.rs` reader 循环中的 `Err(_) => break`
- **问题**: read 出错时静默退出，不记录原因
- **方案**: 通过 channel 把错误传回主线程，或至少记录到 stderr

## 低优先级

### 5. measure_cell_width 重复测量
- **位置**: `src/render.rs` `measure_cell_width()` 的调用方
- **问题**: 字体和字号未变化时也会重复解析字体并测量字符宽度
- **方案**: 缓存到 `AgentTerminal`，在字体、fallback 或字号变化时更新
