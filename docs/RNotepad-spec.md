# RNotepad · 实现规范 v3.0

> 唯一权威文档，直接供 AI coding agent 执行。只描述「做什么 / 怎么做 / 验收」；决策历史与选型论证见 `docs/archive/`，不参与实现。  
> 目标：Windows 10/11 x64，Rust 2021+，轻量文本 / 日志 / 配置查看与编辑工作台。

---

## 1. 范围

**做**：文本文件查看与轻编辑、GB 级日志只读查看与查找、编码 / BOM / EOL 识别与切换、**同一文件在 Text 与 Hex 视图间切换与联动**、查找替换 + 独立结果面板、JSON 格式化 / 压缩 / 错误定位、应用内 dock / 浮动面板、只读 Hex 查看。

**不做**：语法高亮 / 补全 / 多光标、动态插件、目录级全文搜索（P2 再评估）、Hex 就地编辑、大文件编辑 / 撤销 / 替换、文件比较、压缩日志解析、跨平台发行、遥测联网、Lexilla、任何 Scintilla binding crate、Scintilla 静态链接、逐版本跟随上游。

---

## 2. 核心修正：Hex 是视图模式，不是文档类型

需求口径：**「二进制」不是打开二进制文件，而是一些文本文件需要同时以二进制（Hex）方式查看** —— 用于核对 BOM、`0D 0A`、GBK 双字节、UTF-16 码元等原始字节。

|                                                     | `ViewMode::Text` | `ViewMode::Hex`           |
| --------------------------------------------------- | ---------------- | ------------------------- |
| `DocumentKind::Editable`（≤16MB 且可解码）                | Scintilla，可编辑    | 原始字节只读，BOM / EOL / 编码字节高亮 |
| `DocumentKind::LargeReadOnly`（>16MB / 用户强制 / 日志扩展名） | 自绘虚拟只读视图         | **同一个 HexView**，分块读 mmap  |

硬性约束：

1. 只有两种 `DocumentKind`；**不存在 `BinaryDocument`**。无法解码为文本的字节流 → `text_available = false`，强制 `LargeReadOnly`（与文件大小无关）、`Text` 视图禁用、默认 `Hex`，仍是同一个 `Document`。
2. 切换视图**不重新打开文件、不重新解码**；Text 与 Hex 共享 `raw`、`LineIndex`、`CoordMap`、`DocumentId`。
3. 可并排显示：PaneTree 的两个 `Leaf` 指向同一 `DocumentId`，各自持有独立 `ViewMode` 与视口位置；`Command::SetViewMode` 只改活动 Leaf，不改文档默认值；文档关闭时同时解绑。
4. `Editable && dirty` 时 Hex 显示**磁盘字节**，状态栏标注「磁盘版本（有未保存修改）」，保存成功后自动刷新；**不把 Scintilla 缓冲反向编码成字节**。
5. Hex 视图恒只读。保存 / 格式化 / 替换 / EOL 转换只对 `Editable + Text` 生效，由 `DocumentKind` 与 `ViewMode` 双重硬禁用。`Reinterpret`（不改字节、只换解释方式）是例外：只读 / Hex 下仍可用，见 §6.1。
6. 编辑阈值默认 16MB（可配）；`LargeReadOnly` 无任何写路径，不需要 undo、覆盖检测、原子替换、流式事务。

---

## 3. 技术栈、Workspace 与构建资产

```
RNotepad = Win32 外壳 (winsafe + windows) + Scintilla 5.6.9 DLL (仅 Editable/Text)
         + 全 Rust 数据引擎 + 2 个自研视图 (LargeView / HexView)
```

### 3.1 Workspace 与文件级任务清单

```
Cargo.toml                     # workspace: core / win / app
vendor/scintilla-5.6.9/        # 官方源码 zip 解包，锁定不跟随上游
scripts/build-scintilla.ps1    # 调官方 win32 makefile，/MT 静态 CRT，x64(/arm64) 各产一份
scripts/gen_sci_consts.py      # 从 Scintilla.h 的 #define 生成 sci_consts.rs
third_party/Scintilla.dll      # 构建产物，二进制资产入库 / CI 缓存
third_party/License.txt        # HPND 许可原文，随包分发

crates/rnotepad-core/src/
  lib.rs  bytes.rs             # RawBytes(Mmap|Owned) / ByteSource
  encoding.rs                  # Charset / BomState / detect / decode / encode / unmappable 预检
  eol.rs                       # EolDetection / EolWritePolicy / 规范化与还原
  line_index.rs                # ChunkIndex + LRU ChunkLineCache
  coord.rs                     # 三套 newtype + CoordMap
  search.rs                    # SearchQuery / SearchService / MatchHit / TaskEvent
  format.rs                    # FormatProvider / Diagnostic / SourceMap / JSON provider
  document.rs                  # Document / DocumentId / DocumentKind / ViewMode / WritePolicy

crates/rnotepad-win/src/
  sci_consts.rs                # 生成物，禁止手改
  sci.rs                       # LoadLibraryW + CreateWindowExW + SendMessageW(SCI_*) + SCNotification
  large_view.rs                # DirectWrite 等宽整行绘制、垂直虚拟化、Follow Tail
  hex_view.rs                  # Offset | 16B Hex | 字符预览，只读
  panes.rs                     # PaneTree / splitter / dock / float
  results.rs                   # SysListView32 + LVS_OWNERDATA
  statusbar.rs                 # SB_SETPARTS 分区、NM_DBLCLK 命中、编码/EOL 就地修改菜单
  dw.rs                        # DirectWrite 封装
  save.rs  notify.rs           # 原子保存 / 外部变更检测

crates/rnotepad-app/src/
  main.rs  main_window.rs      # 消息循环、主窗口
  session.rs  commands.rs      # Session 状态机、Command 分发
  tasks.rs                     # TaskPool、后台任务、PostMessage 回灌
```

依赖方向：`core` 不 import 任何 OS / UI 类型；`win` 不 import `app`；`app` 单向依赖前两者。

### 3.2 Cargo 依赖（P0）

| Crate                                                  | 用途                                              |
| ------------------------------------------------------ | ----------------------------------------------- |
| `winsafe` (`gui`,`comctl`,`user`,`gdi`,`dwm`,`kernel`) | 窗口 / 控件 / 消息循环安全封装                              |
| `windows`                                              | DirectWrite / DPI Awareness / `MoveFileExW` 补充  |
| `memmap2`                                              | 只读映射                                            |
| `encoding_rs`                                          | GB18030 / GBK / Big5 / Shift-JIS / Windows-1252 |
| `memchr`                                               | 字面量字节搜索快路径                                      |
| `regex`                                                | 正则（线性时间）                                        |
| `serde_json`                                           | `preserve_order` + `arbitrary_precision`        |
| `unicode-width`                                        | 展示列宽                                            |
| `crossbeam-channel`                                    | 后台任务批量回灌                                        |
| `thiserror` / `tracing`                                | 错误分类 / 落盘日志                                     |

P1：`notify`（外部追加 / 修改检测）、`chardetng`（无 BOM 统计检测）。P2：`serde_norway` / `yaml-rust2`、`toml`、`quick-xml`。  
不引入：egui、cosmic-text、ropey、rayon、parking_lot、async runtime、rfd、aho-corasick、`scintilla-sys`。

### 3.3 Scintilla 接入约定

- 官网只发源码、无独立 DLL 下载 → 从 `vendor/scintilla-5.6.9/` 用 `scripts/build-scintilla.ps1` 编译**不含 lexer** 的 `Scintilla.dll`；不从 SciTE 包取 DLL。
- `cargo build` 必须保持纯 Rust，不依赖 C++ 工具链；DLL 作为二进制资产加载（`LoadLibraryW`）。
- `sci_consts.rs` 由脚本生成并随 vendor 版本提交；升级 = 换 vendor + 重跑脚本 + 过 §8 编码矩阵。
- `SCI_SETCODEPAGE(SC_CP_UTF8)` 后 Scintilla position 即 `Utf8Pos`；`SCI_SETEOLMODE` / `SCI_CONVERTEOLS` / `SCI_GETLINEENDPOSITION` 用于 EOL 三态。
- `sci.rs` 约 200–300 行，`unsafe` 仅允许出现在 `LoadLibraryW`、`SendMessageW` 指针参数、`SCNotification` cast 三处。
- `SCI_GETDIRECTFUNCTION` / `SCI_GETDIRECTPOINTER` 跨线程不安全，不使用。
- 分发物：`RNotepad.exe + Scintilla.dll + License.txt`。

**验收**：干净机器执行 `scripts/build-scintilla.ps1` 产出可加载 DLL；`cargo build --release` 全程不需要 MSVC；P0-1 编码矩阵全绿、中文 IME 可输入。

---

## 4. 数据模型与接口签名

```rust
#[derive(Copy, Clone)] pub struct RawByteOffset(pub u64);
#[derive(Copy, Clone)] pub struct Utf8Pos(pub usize);
pub struct ViewPos { pub line: u32, pub col: u32 }
pub struct Span<T> { pub start: T, pub end: T }
pub type DocumentId = u64;

pub enum DocumentKind { Editable, LargeReadOnly }
pub enum ViewMode { Text, Hex }
pub enum Charset { Utf8, Utf16Le, Utf16Be, Gb18030, Gbk, Big5, ShiftJis, Windows1252, Latin1 }
pub enum BomState { None, Utf8, Utf16Le, Utf16Be }
pub enum BomPolicy { Keep, Add, Strip }
pub enum EolKind { CrLf, Lf, Cr }
pub enum EolWritePolicy { Preserve, CrLf, Lf, Cr }
pub struct EolDetection { pub crlf: u64, pub lf: u64, pub cr: u64,
                          pub dominant: EolKind, pub mixed: bool }
pub struct WritePolicy { pub charset: Charset, pub bom: BomPolicy, pub eol: EolWritePolicy }
pub struct EncodingInfo { pub charset: Charset, pub bom: BomState, pub policy: BomPolicy }

pub struct Document {
    pub id: DocumentId,
    pub path: Option<PathBuf>,
    pub raw: RawBytes,                  // Mmap | Owned(Vec<u8>)，唯一真相
    pub kind: DocumentKind,
    pub view: ViewMode,                 // 当前活动视图；并排时每个 Leaf 各持一份
    pub text_available: bool,           // false -> Text 视图禁用，默认 Hex
    pub encoding: EncodingInfo,         // charset + bom + policy
    pub eol: EolDetection,
    pub write_policy: WritePolicy,      // 仅 Editable 有效
    pub line_index: Arc<LineIndex>,
    pub coord: CoordMap,
    pub dirty: bool,                    // 仅 Editable 有效；LargeReadOnly 恒 false
    pub revision: u64,
}

pub trait ByteSource {
    fn len(&self) -> u64;
    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<usize>;
    fn slice(&self, r: Range<u64>) -> Option<&[u8]>;
}
pub trait CoordMap {
    fn raw_to_utf8(&self, r: RawByteOffset) -> Option<Utf8Pos>;
    fn utf8_to_raw(&self, p: Utf8Pos) -> Option<RawByteOffset>;
    fn raw_to_view(&self, r: RawByteOffset) -> Option<ViewPos>;
}
pub struct LineIndex { /* ChunkIndex + LRU ChunkLineCache + version */ }
impl LineIndex {
    pub fn line_start(&self, line: u32) -> Option<RawByteOffset>;
    pub fn line_of(&self, off: RawByteOffset) -> Option<u32>;
    pub fn append_from(&mut self, off: RawByteOffset);
}

pub enum SearchMode { Literal, Regex, HexBytes }
pub struct SearchQuery { pub pattern: String, pub mode: SearchMode,
                         pub case_sensitive: bool, pub whole_word: bool }
pub struct MatchHit { pub doc_id: DocumentId, pub raw_offset: u64, pub match_len: u32,
                      pub line_no: u32, pub col: u32, pub doc_revision: u64 }
pub enum TaskEvent { Progress(u64, u64), Hits(Vec<MatchHit>), Done, Failed(String) }
pub trait SearchService {
    fn run(&self, q: &SearchQuery, d: &Document, cancel: CancelToken,
           sink: Sender<TaskEvent>) -> Result<SearchSummary>;
}
pub enum ReplacePlan { /* 仅 Editable+Text */ }

pub trait FormatProvider {
    fn kind(&self) -> FormatKind;
    fn validate(&self, s: &str) -> Result<(), Vec<Diagnostic>>;
    fn format(&self, s: &str, o: &FormatOptions) -> Result<FormatOutput, Vec<Diagnostic>>;
    fn minify(&self, s: &str) -> Result<FormatOutput, Vec<Diagnostic>>;
}
pub struct Diagnostic { pub span: Span<Utf8Pos>, pub line_col: (u32, u32),
                        pub message: String, pub hint: Option<String> }
pub struct FormatOutput { pub text: String, pub source_map: SourceMap }

pub enum ViewCommand {
    Goto(Utf8Pos), Highlight(Span<Utf8Pos>),
    SetReadOnly(bool), SetViewMode(ViewMode), Selection,
}
pub enum PaneTree {
    Split { axis: Axis, ratio: f32, first: Box<PaneTree>, second: Box<PaneTree> },
    Leaf(PaneId),   // TextView | HexView | Results | Problems
}
pub enum StatusField { View, Encoding, Eol, Size, Position, Flags, Offset, Progress }
pub enum Command {
    Open(PathBuf), SetViewMode(ViewMode), SplitView(ViewMode),
    Reinterpret(Charset), ConvertTo(Charset), SetBom(BomPolicy),
    SetEol(EolWritePolicy),   // 仅设写入策略，保存时生效，不改当前缓冲
    ConvertEols(EolKind),     // 立即转换当前缓冲，dirty=true，可一次撤销
    Find(SearchQuery), ReplaceAll(ReplacePlan),
    Format(FormatKind), Minify(FormatKind), Save(Option<PathBuf>),
    GotoOffset(RawByteOffset), Task(TaskEvent),
}
pub struct Session { /* docs, tasks, panes */ }
impl Session { pub fn dispatch(&mut self, cmd: Command); }
```

`SearchScope { CurrentDoc, AllOpenDocs, Directory }` 与 `MatchHit.doc_id` 为 P2 目录搜索预留，首版只实现 `CurrentDoc`。  
未列出的辅助类型（`RawBytes`、`CancelToken`、`SearchSummary`、`FormatKind`、`FormatOptions`、`SourceMap`、`Axis`、`PaneId`、`DecodeError`）按上述语义在 `core` 内定义，不新增外部依赖。

---

## 5. 坐标与行索引

- 三套 newtype 强隔离：`RawByteOffset`（原始文件字节）→ 行索引 / 结果条目 / Hex / 持久化；`Utf8Pos`（解码后 UTF-8 字节偏移）→ Scintilla position、`serde_json::Error::column()`；`ViewPos` → 状态栏 / 光标 / 跳转。跨层换算一律经 `CoordMap`。
- `CoordMap` = 4KB 稀疏锚点 + 块内线性换算，不建全量映射表；UTF-16 换行符字节为 `0A 00` / `00 0A`，行扫描由 `newline_tokens + align` 参数化；UTF-16 BOM、代理对、码元→UTF-8 由自研 codec 处理。
- 两级行索引：`ChunkIndex` 每 64KB 一个 `(raw_offset, line_no)`（1GB ≈ 1.6 万项 ≈ 256KB）；`ChunkLineCache` LRU 缓存已访问块内行起点，上限 ~32MB。
- 打开时先建 1MB 采样粗索引让首屏立即可渲染，后台补全；索引绑定 `(raw_revision, charset, eol_tokens)`；切换后用「行号 + 行内前 64 字节哈希」锚点还原视口；日志追加只从旧末尾增量扫描。

---

## 6. 子系统实现规范


### 6.1 编码与 BOM

- 检测链：BOM 精确 → UTF-8 合法性快路径 → `chardetng` + 中文分布启发式 → 低置信度时状态栏显示「猜测：GB18030 (62%) ▾」并记住「扩展名 / 目录 → 编码」偏好。
- `struct EncodingInfo { charset, bom, policy }`，BOM 与 charset 独立；「立即加上 / 去掉 BOM」只改文件头 3 字节，仅 `Editable + Text` 可用。
- **两个动作严格分开**：`Reinterpret`（不改字节，换解释方式；**对 `LargeReadOnly` 与 Hex 同样可用**，只重建视图；对 `Editable` 会重建 Scintilla 文档并清空 undo → 有 dirty / undo 历史时二次确认）与 `ConvertTo`（改字节，保存时生效，仅 `Editable + Text`）。
- `ConvertTo` 前预检 unmappable 字符列表，存在则**禁止保存**；解码失败用 U+FFFD 并记录 `DecodeError { raw_offset }`，点击跳 Hex 视图定位原始字节。
- `text_available = false` 时禁用 Text 视图、显示 Hex，并在状态栏给出原因。

### 6.2 换行符

- `EolDetection` 统计 `crlf/lf/cr` 与 `mixed`；`Mixed` 是**检测结果**，不是可写目标。
- 打开时不改任何字节，视图统一按逻辑行规范化；Mixed 显式告警并给统计。
- `SetEol(EolWritePolicy)` 只设写入策略（保存时生效，不改当前缓冲）；`ConvertEols(EolKind)` 调 `SCI_SETEOLMODE` + `SCI_CONVERTEOLS` **立即改当前缓冲**，`dirty = true`，一次 Ctrl+Z 可撤销。两者仅 `Editable + Text` 可用；`LargeReadOnly` 与 Hex 只做视图规范化，不提供 EOL 转换。
- 查找中的 `^ $ \R` 基于逻辑行语义，与物理 EOL 无关。

### 6.3 三种视图

**TextView（Editable）**：Scintilla 子 HWND，UTF-8 codepage，编辑 / undo / IME / F3 就地高亮由控件提供；Rust Core 只负责「原始字节 ⇄ UTF-8」与保存。

**LargeView（LargeReadOnly / Text）**：DirectWrite 等宽整行绘制，只画可见行；无 IME、无可编辑选区、无字符级光标、无 undo；超长单行按显示列切片，禁止整行布局；Follow Tail 增量刷新。

**HexView（任意 Document / 只读）**：

- 数据源直接是 `raw`，**不经任何字符解码**；`LargeReadOnly` 按 1MB 分块读 mmap，视口外不缓存。
- 布局 `Offset(8 hex) | 16 字节 Hex | 字符预览`；字符预览：可打印 ASCII 直接显示，非 ASCII 按当前 `charset` 尝试解码，不可映射显示 `.`。
- 高亮：`BomState` 对应的头部字节、`0D 0A` / `0A` / `0D` 的 EOL 字节、当前 charset 的多字节序列用底色分组；状态栏显示当前编码 / BOM / EOL 统计。
- 能力：按偏移跳转、Hex 字节串与文本字面量互搜（后者按当前 charset 编码）、选区复制为 `0xAA 0xBB` / `AA BB` / C 数组、与 Text 视图双向联动。
- 联动：Text 光标 / 选区 `Utf8Pos` → `CoordMap::utf8_to_raw` → Hex 高亮 `[start, end)`；Hex 选中 `RawByteOffset` → `raw_to_utf8` → `ViewCommand::Goto` 定位文本。非 UTF-8 下一个字符对应多字节时，Hex 高亮整个码元范围。
- 任何情况下 HexView 不提供写入口；`Editable && dirty` 显示磁盘版本并标注。

### 6.4 查找替换与结果面板

- **唯一的 `SearchService` 在 Core**，跨两种 `DocumentKind` 与两种 `ViewMode`；Scintilla 内置查找只用于 Editable 的 F3 就地高亮，不承担 Find All。
- `SearchMode::HexBytes` 解析形如 `DE AD BE EF` / `\xDE\xAD` 的字节串，直接在 `raw` 上用 `memchr` 匹配；结果与文本搜索同为 `MatchHit`，结果面板模型不变。
- 后台线程 + `AtomicBool` 取消 + 进度；按 1MB 分块、块尾保留 `max_pattern_len-1` 重叠；正则按逻辑行分块；结果按「200 条或 16ms」批量经有界 channel + `PostMessage` 回灌。
- 结果面板是应用内 dock / 浮动面板，`SysListView32` 的 `LVS_OWNERDATA`（`LVN_GETDISPINFO` 按需取文本）—— 10 万条近零内存；`MatchHit` 不存 `PathBuf`、不存 preview 字符串；`doc_revision` 变化即标记 stale。
- 双击跳当前活动视图并高亮；右键「在 Hex 中显示 / 在 Text 中显示」切换目标视图。
- 替换：**仅 `Editable + Text`**，Replace All 是单个 Scintilla 事务，一次 Ctrl+Z 撤销；Large / Hex 的替换入口直接禁用。

### 6.5 格式化与错误定位

- JSON 用 `serde_json`（`preserve_order` 不重排 key、`arbitrary_precision` 保大整数）；`Error::line()/column()` 是 UTF-8 字节列，经 `CoordMap` 转 `RawByteOffset` 与展示列（中文 / Emoji 下不同，不可直接当位置用）。
- **失败不产生新文档**：原视图按 span 画标记 + 问题面板列出原因 / 建议；先生成完整新内容并验证成功才允许替换。
- Lenient 模式（`//` 注释、尾逗号、单引号）用**等长空格替换**预处理，保证偏移不漂移。
- 格式化属编辑操作，**仅 `Editable + Text`**；P2 增 YAML / TOML / XML 只实现 `FormatProvider` + 注册。

### 6.6 窗口与面板

- `PaneTree`：每个 `Leaf` 是子 HWND，主窗口 `WM_SIZE` 时递归 `SetWindowPos`，splitter 拖动改 `ratio`；布局序列化到 `config.toml`。
- 「浮动」= `SetParent(pane, float_frame)`，宿主为 `WS_POPUP` + owner=主窗口的工具窗口（跟随最小化、不进任务栏）；关闭时 `SetParent` 回主窗口；共享同一 `Session` 状态，不复制视图实例。
- 首版只开放「主视图 | 右侧面板（Results / Problems 分 Tab）」两栏，以及「主视图 | Hex 并排」这一预设；PaneTree 保留嵌套能力但不开放无限自定义。
- 状态栏字段、双击就地修改见 §6.7。

### 6.7 状态栏与就地修改

**分区**：`SysStatusBar32` + `SB_SETPARTS`（索引从 0，最后一项 `-1` 延伸到右边缘；`WM_SIZE` 与 DPI 变化时重算宽度）。分区常驻，无内容时留空。

| # | `StatusField` | 示例                              | 双击             |
| - | ------------- | ------------------------------- | -------------- |
| 0 | `View`        | `TEXT` / `HEX`                  | 否              |
| 1 | `Encoding`    | `UTF-8 · BOM` / `GB18030 (62%)` | **是 → 编码菜单**   |
| 2 | `Eol`         | `CRLF` / `LF` / `Mixed 1204:37` | **是 → EOL 菜单** |
| 3 | `Size`        | `1.24 GB` / `16,384 B`          | 否              |
| 4 | `Position`    | `Ln 12, Col 34` / `Sel 128 B`   | 否              |
| 5 | `Flags`       | `RO` / `EDIT *` / `磁盘版本`        | 否              |
| 6 | `Offset`      | `0x0000A1F0`（仅 Hex / 并排时）       | 否              |
| 7 | `Progress`    | `Find All 42%`（仅任务期间）           | 否              |

- 可双击字段文本带 `▾` 后缀，`SB_SETTIPTEXT` 提示「双击修改」；`NM_DBLCLK` 经 `WM_NOTIFY` 送达父窗口，`NMMOUSE.dwItemSpec` 为分区索引，处理完返回 `TRUE` 抑制默认处理。
- 弹出菜单：`CreatePopupMenu` + `TrackPopupMenuEx(TPM_RETURNCMD | 对齐标志)`，对齐按 `SM_MENUDROPALIGNMENT`，坐标用 `GetCursorPos` / `ClientToScreen` 转屏幕坐标；菜单 ID **从 1 起**，返回值映射为 `Command`，最后 `DestroyMenu`。
- 菜单弹出期间 `Session` 暂存 `TaskEvent` 到待处理队列，菜单关闭后统一 dispatch，避免文档在菜单打开时被后台任务改动导致命令目标过期。
- **不弹设置对话框、不进二级页面**：选中即执行，状态栏与视图立即刷新。

**编码菜单（`StatusField::Encoding`）**：顶部显示当前检测结果与置信度，三段分组：

1. `重新解释（不改字节）` → `Command::Reinterpret(Charset)`，列出 `Charset` 全部 9 个变体；**`LargeReadOnly` / Hex 下同样可用**，执行后立即重建视图或 Scintilla 文档。
2. `转换为（改字节，保存时生效）` → `Command::ConvertTo(Charset)`，同一列表；非 `Editable + Text` 时整组 `MF_GRAYED`。
3. `BOM` → `保留 / 添加 / 去除` → `Command::SetBom(BomPolicy)`；非 `Editable + Text` 时灰显。UTF-8 是否带 BOM 由此段控制，状态栏显示 `UTF-8 · BOM` / `UTF-8`。

当前项 `MF_CHECKED`；`Reinterpret` 在 `dirty` 或存在 undo 历史时先弹一次轻量确认（会清空 undo），否则直接执行。`ConvertTo` 执行后状态栏立即显示目标编码、`dirty = true`、写入策略切到目标 charset；unmappable 预检失败则中止并保留原状态。

**EOL 菜单（`StatusField::Eol`）**：顶部显示 `CRLF / LF / CR` 计数与 `Mixed` 标记，两段分组：

- `保持原样（Preserve）` → `SetEol(Preserve)`；检测到 `Mixed` 时默认勾此项。
- `立即转换为 Windows (CRLF) / Unix (LF) / Classic Mac (CR)` → `ConvertEols(kind)` 并同步 `SetEol(kind)`；在单个 Scintilla 事务内完成，`dirty = true`，一次 Ctrl+Z 可撤销，状态栏立即更新。
- `LargeReadOnly` / Hex：转换项全部 `MF_GRAYED`，只显示统计与「只读，无转换」提示项。

|                                                                                |                                                                                                                                                                                         |
| ------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **只读拦截（双保险）**：菜单构建时按 `(DocumentKind, ViewMode, dirty)` 计算可用项，禁用项一律 \`MF_GRAYED | MF_DISABLED`；`Session::dispatch`对`ConvertTo / SetBom / ConvertEols`以及`SetEol`的非`Preserve`值再做一次硬校验（防菜单状态过期），不满足返回`Err(ReadOnly)` 并在状态栏闪一条提示。`Reinterpret`与`SetEol(Preserve)\` 不改字节，不受此限。 |

---

## 7. 线程模型与安全门禁

- UI 线程只跑消息循环；索引、查找、格式化、编码转换、保存全进后台线程；完成用 `PostMessage(WM_APP_TASK_*)` 回灌；任务与 `DocumentId` 绑定，文档关闭即取消。
- `notify` 检测外部变更：`LargeReadOnly` 恒未 dirty → 增量重映射 + Follow Tail；`Editable` 已 dirty → 横幅提示「重新加载 / 保留我的版本」。日志正在写入时不得映射超出当前文件 size。
- 保存仅作用于 `Editable`：比对 size + mtime 检测外部修改 → 写同目录临时文件 → **释放 mmap 与所有文件句柄** → `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` → 可选 `.bak`。

| 门禁                 | 实现                                                |
| ------------------ | ------------------------------------------------- |
| 查看默认只读             | `LargeReadOnly` 与 Hex 强制只读且无写路径；`Editable` 显式切换编辑 |
| 未保存状态可见            | 标题栏 `*` + 关闭确认（仅 Editable）                        |
| 保存前外部修改检测          | size + mtime + 可选 hash                            |
| 原编码 / EOL / BOM 保持 | `WritePolicy::Preserve` 为默认                       |
| 转码失败禁止静默保存         | 预检 unmappable 列表并中止                               |
| 大任务可取消             | `AtomicBool` + 进度 + 关闭文档即取消                       |
| 格式化失败不覆盖原文         | §6.5                                              |
| Hex 不误走文本保存        | `ViewMode::Hex` 在保存入口硬拦截                          |
| 状态栏就地修改只读拦截        | §6.7 菜单灰显 + `Session::dispatch` 二次硬校验             |

---

## 8. 里程碑与验收

| 阶段       | 交付                                                                                    | 验收重点                                                                                            |
| -------- | ------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| **P0-1** | `scripts/*`、Win32 骨架、Scintilla 薄封装、Editable 打开、编码 / BOM / EOL 检测、**状态栏分区 + 双击就地修改菜单** | 编码矩阵全绿；GBK / UTF-16LE 不乱码；中文 IME 可输入；`cargo build` 不需 C++ 工具链；**双击编码 / EOL 一键生效，只读项灰显且命令层二次拦截** |
| **P0-2** | 大文件引擎（mmap + 两级索引 + LargeView）、**HexView（任意文档可切）**、Follow Tail、并排视图                   | 1GB 首屏 <1s；RSS 增量 <150MB；Hex 字节与磁盘完全一致；BOM/EOL 高亮正确；Text⇄Hex 双向联动精确；无写入口                        |
| **P0-3** | SearchService（含 HexBytes）、Find All、Editable 替换、dock / 浮动结果面板                          | 500MB Find All 可取消；10 万条结果不涨内存；双击 / 右键跳转精确；面板可停靠 / 浮动                                           |
| **P0-4** | JSON format / minify / validate、错误定位、问题面板                                             | 错误行列精确（含中文 / Emoji）；失败不覆盖原文；大整数精度不变                                                             |
| **P1**   | Editable 编辑 / 保存 / 原子替换、外部修改检测、多标签与会话恢复                                               | 数据一致性；保存后编码 / EOL 保持；dirty 时 Hex 显示磁盘版本并在保存后刷新                                                  |
| **P2**   | YAML / TOML / XML、目录级搜索                                                               | 新增格式只改 `FormatProvider` 实现；目录搜索复用现有结果模型                                                         |

**质量门禁**：proptest 随机字节流验证 decode / index / coord 不 panic 且往返自洽；golden 样本（GBK 日志、UTF-16LE 无 BOM、混合 EOL、单行 10MB JSON、4GB+ 稀疏文件、含 `0x00` 的文本、纯二进制流）；criterion 基准（1GB 首屏、索引吞吐、Find All 吞吐）；全链路 `u64` 偏移。

**Hex 专项验收**：`EF BB BF` + 中英混排 + CRLF/LF 混合样本，逐字节比对 Hex 与文件；UTF-16LE 无 BOM 样本中 `0A 00` 不被误判为行尾；dirty 文档切 Hex 显示磁盘版本且标注存在；并排时两个 Leaf 滚动互不干扰、关闭文档同时解绑。

---

## 9. 已知实现坑位

1. 原子替换前必须释放 mmap 与所有文件句柄，否则 Windows 上 `MoveFileExW` 失败。
2. `SendMessageW` 返回值可能为 64 位指针（`SCI_GETDIRECTPOINTER`），禁止用 32 位类型截断。
3. Scintilla `DirectFunction` 跨线程不安全；所有 `SCI_*` 调用限 UI 线程。
4. 切换编码 / 重新解码会重建 Scintilla 文档并清空 undo，UI 需二次确认。
5. `regex` 不支持 look-around 与反向引用，UI 明示；不引入 `fancy-regex`。
6. `encoding_rs` 的 GBK / GB18030 按 WHATWG 映射，与 Windows CP936 存在个别差异，须以真实日志样本回归。
7. UTF-16 行扫描必须带 `align` 参数，否则 `0A 00` / `00 0A` 会错位；UTF-16/32 BOM、代理对需自研 codec。
8. DPI：Per-Monitor V2 下 `SetWindowPos` 用物理像素；HexView 列宽按 DPI 缩放。
9. 并排 Text/Hex 两个 Leaf 共享 `DocumentId`，文档关闭必须同时解绑，避免悬空 HWND 回调。
10. HexView 对 `LargeReadOnly` 采用分块读取，禁止为整个文件分配字节缓存。
11. 状态栏 `NM_DBLCLK` 由 `WM_NOTIFY` 送达**父窗口**，`NMMOUSE.dwItemSpec` 是分区索引；`SB_SETPARTS` 的 `-1` 分区、`WM_SIZE` 重算、DPI 变化必须同步，否则双击命中错字段。
12. `TrackPopupMenuEx(TPM_RETURNCMD)` 的菜单 ID 必须从 1 起（0 与「取消返回 0」冲突）；坐标必须是屏幕坐标；`CreatePopupMenu` 与 `DestroyMenu` 必须配对。
13. 状态栏就地改编码会重建 Scintilla 文档并清空 undo，`dirty` 时须先确认；`ConvertEols` 必须在单个 Scintilla 事务内完成，保证一次 Ctrl+Z 撤销。
