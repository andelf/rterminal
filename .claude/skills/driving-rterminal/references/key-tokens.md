# Key token grammar for `POST /tabs/:id/keys`

The body is plain UTF-8 text. The parser is whitespace-delimited and atomic — if any token is invalid the whole request returns 400 with no bytes written.

## Token types

There are three kinds of tokens:

1. **Named key**, optionally with modifiers — e.g. `Enter`, `C-c`, `M-Up`, `S-Tab`.
2. **Single literal character**, optionally with modifiers — e.g. `a`, `!`, `C-x`.
3. **Quoted literal string** — `"…"` — content is inserted verbatim, including spaces.

## Named keys

| Token | Bytes |
|---|---|
| `Enter` | `\r` (0x0D) |
| `Tab` | `\t` (0x09) |
| `Escape` | `\x1b` |
| `Space` | ` ` |
| `BSpace` | `\x7f` (backspace / DEL) |
| `Up` | `\x1b[A` |
| `Down` | `\x1b[B` |
| `Right` | `\x1b[C` |
| `Left` | `\x1b[D` |
| `Home` | `\x1b[H` |
| `End` | `\x1b[F` |
| `PageUp` | `\x1b[5~` |
| `PageDown` | `\x1b[6~` |
| `F1` | `\x1bOP` |
| `F2` | `\x1bOQ` |
| `F3` | `\x1bOR` |
| `F4` | `\x1bOS` |
| `F5` | `\x1b[15~` |
| `F6` | `\x1b[17~` |
| `F7` | `\x1b[18~` |
| `F8` | `\x1b[19~` |
| `F9` | `\x1b[20~` |
| `F10` | `\x1b[21~` |
| `F11` | `\x1b[23~` |
| `F12` | `\x1b[24~` |

These are case-sensitive — `enter` is not the same as `Enter` (and will be parsed as a 5-character literal instead, which is almost certainly not what you want).

## Modifiers

Three modifiers, applied as `-`-separated prefixes:

| Prefix | Modifier | Notes |
|---|---|---|
| `C-` | Ctrl | Combines with letters (A-Z, a-z, `@`, `[`, `\`, `]`, `^`, `_`) to produce ASCII control bytes. Case-insensitive in the letter (`C-a` == `C-A` == 0x01). Cannot combine with most named keys. |
| `M-` | Meta / Alt | Prepends an ESC byte (`0x1b`) to whatever follows. Works with anything. |
| `S-` | Shift | Only meaningful with `Tab` (produces back-tab `\x1b[Z`). Rejected for letters — use the uppercase letter directly. |

### Ordering

Any order is accepted: `C-M-x` and `M-C-x` both produce `\x1b\x18` (ESC + Ctrl-X). Duplicates within a single token are rejected (`C-C-x` → 400 error).

### Ctrl byte math

For letters, `C-X` produces the byte `X.to_ascii_uppercase() - '@'`. So:
- `C-@` → 0x00 (NUL)
- `C-a` / `C-A` → 0x01 (SOH)
- `C-c` → 0x03 (ETX, the SIGINT byte)
- `C-i` → 0x09 (HT, same as Tab — but be explicit and use `Tab`)
- `C-m` → 0x0D (CR, same as Enter)
- `C-u` → 0x15 (NAK — terminal "kill line")
- `C-x` → 0x18 (CAN)
- `C-z` → 0x1A (SUB)
- `C-[` → 0x1B (ESC)

Useful pairings:
- `C-c` — interrupt the foreground job (SIGINT)
- `C-d` — EOF / logout
- `C-u` — clear input line back to the prompt
- `C-l` — clear screen and redraw
- `C-r` — incremental history search
- `C-w` — delete previous word

### Meta examples

| Token | Bytes | Common use |
|---|---|---|
| `M-b` | `\x1bb` | Move cursor word back (bash/zsh readline) |
| `M-f` | `\x1bf` | Move cursor word forward |
| `M-Backspace` | n/a — use `M-BSpace` → `\x1b\x7f` | Delete word backward |
| `M-.` | `\x1b.` | Insert last argument of previous command |

## Single-character literals

Any printable ASCII character that isn't reserved can stand alone as a token. Examples: `a`, `Z`, `7`, `!`, `?`, `/`, `=`. UTF-8 single-character literals also work, but reach for the quoted form for anything multi-byte to avoid ambiguity.

To send a literal `-`, `"`, or `\\`, wrap it in quotes: `"-"`, `"\""`, `"\\"`.

## Quoted literals

Anything inside `"…"` is inserted verbatim. Two escape sequences are recognised inside the quotes:

| Escape | Result |
|---|---|
| `\"` | `"` (literal double-quote) |
| `\\` | `\` (literal backslash) |

No other escapes are interpreted — `\n` inside a quoted string is literally a backslash followed by `n`, not a newline. Use the `Enter` named key for newlines:

```
"echo hi" Enter
```

Unterminated quotes (`"oops`) → 400 error.

## Composition examples

| Body | Bytes sent | Effect |
|---|---|---|
| `Enter` | `\r` | Submit current line |
| `C-c` | `\x03` | SIGINT |
| `C-l` | `\x0c` | Clear screen |
| `"ls -la" Enter` | `ls -la\r` | Run `ls -la` |
| `C-a "echo " Up Enter` | `\x01echo \x1b[A\r` | Go to start of line, prepend "echo ", arrow up… (illustrative) |
| `Up Up Enter` | `\x1b[A\x1b[A\r` | Run the command two slots back in history |
| `C-u "cd /tmp" Enter` | `\x15cd /tmp\r` | Clear any partial input, run `cd /tmp` |
| `M-b M-b C-w` | `\x1bb\x1bb\x17` | Jump back two words, then delete word — readline edit |
| `Escape Escape` | `\x1b\x1b` | Two raw escapes — vim's "back to normal mode" depending on state |

## Atomicity guarantee

The parser splits the entire body into tokens before writing anything to the PTY. If any token is malformed — unknown name, bad modifier combination, unterminated quote — the request returns 400 with an error message and **zero bytes are written**. You won't end up with "half the keystrokes" on a parse error.

This means you can safely batch a long sequence in one request without worrying about partial state.

## Errors

| Body | Status | Error |
|---|---|---|
| `Foo` | 400 | `unknown key token: Foo` |
| `C-C-x` | 400 | `duplicate Ctrl modifier in 'C-C-x'` |
| `S-a` | 400 | `Shift modifier not supported with 'a' in 'S-a'` |
| `C-Enter` | 400 | `Ctrl modifier not supported with named key 'Enter'` |
| `"oops` | 400 | `unterminated quoted string` |
| `"\n"` | 400 | `invalid escape: \n` |

When a request fails, fix the typo and re-send the whole thing — the failure is total, not partial.
