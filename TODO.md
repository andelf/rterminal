# rterminal 待办改进清单

## 高优先级

### 1. 逐字符渲染 → 批量按行 shape
- **位置**: `src/render.rs` canvas 闭包，每个非空白字符单独调用 `shape_line()`
- **问题**: 80×24 终端每帧最多 1920 次 shape API 调用
- **方案**: 按行拼接文本后批量 shape，一次绘制整行，减少 GPU 文本排版开销

## 中优先级

### 2. rewrite_terminal_input_line 宽字符光标定位
- **位置**: `src/input.rs` `rewrite_terminal_input_line()`
- **问题**: 通过 N 次 `\x1b[D` 移动光标，中文等宽字符占两列会错位
- **方案**: 计算字符实际列宽（wcwidth），按列数发送左箭头

### 3. HTTP 调试接口安全加固
- **位置**: `src/terminal.rs` `start_debug_http_server()` 无条件启动
- **问题**: 默认监听 localhost:7878，`/debug/input` 可注入任意字节
- **方案**: 默认不启动，需 `--debug-http` 显式开启

### 4. Debug Server 生命周期治理
- 详见 `docs/IMPROVEMENT_PLAN.md` 阶段 3

## 低优先级

### 5. indexed_to_rgb 传入正确 colors
- **位置**: `src/color.rs:104-111`
- **问题**: 索引 0-15 每次用 `Default::default()` Colors，忽略终端自定义主题色
- **方案**: 将当前终端 colors 传入 `indexed_to_rgb`（目前调用处已传，函数内未用）

### 6. measure_cell_width 缓存
- **位置**: `src/render.rs` canvas paint 闭包
- **问题**: 每帧重新测量字符宽度，值仅在字体/大小变化时才变
- **方案**: 缓存到 AgentTerminal 字段，`adjust_font_size` / `sync_grid_to_window` 时更新

### 7. PTY reader 线程错误静默
- **位置**: `src/pty.rs:65` `Err(_) => break`
- **问题**: read 出错时静默退出，不记录原因
- **方案**: 通过 channel 把错误传回主线程，或至少 eprintln

## 功能缺失

### 8. Cmd+C 复制支持
- 已实现选区（Shift+拖拽），但缺少 Cmd+C 将选区复制到剪贴板

### 9. input_line 与 shell 状态同步
- **位置**: `src/input.rs` `apply_terminal_bytes_to_input_line()`
- **问题**: 未处理 Up/Down 历史导航、Tab 补全等场景，方向键后 input_line 与实际 shell 输入不一致
