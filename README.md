# 晒窗 · ShaiWin

**常用的窗口挂起来，随点随用。**

![四张窗口卡片挂在晾衣绳上](assets/usage.png)

---

## 软件介绍

晒窗是一个 Windows 桌面小工具，只做一件事：**把你正在用的窗口从屏幕上「摘下来」，挂到屏幕顶端的一根晾衣绳上。**

抓住任意窗口的标题栏往屏幕顶端一拖，它就像被吸上去一样缩成一张卡片，垂在绳子上；点一下卡片，它长回原来的大小。桌面立刻干净，但窗口没关、也没从任务栏和 Alt+Tab 里消失：它是**原地最小化**挂在架上的，任务栏按钮点一下就回桌面——就算晒窗自己崩了或被强杀，你也永远找得回它。

它和窗口管理器的关系是**克制**的：

- 只挂你亲手挂上去的窗口，不接管、不整理、不猜测。
- 不隐藏任务栏按钮，不改动 Alt+Tab，不动系统的 Snap 布局。
- 单个 exe，绿色免安装，不需要管理员权限。

适合的场景：日常工作中运行的软件窗口过多，多窗口无法快速定位。

---

## 下载与构建

### 方式一：直接下载（推荐）

到 [Releases](https://github.com/19y/ShaiWin/releases/latest) 下载最新的 `shaiwin-<版本>-win64.zip`，解压到任意目录，双击 `shaiwin.exe` 就能用。

- 单个 exe，绿色免安装，不需要 .NET / VC++ 运行库，不需要管理员权限。
- 压缩包里只有 `shaiwin.exe`、`LICENSE`、`README.md` 三个文件；卸载 = 删掉目录 + 删掉 `%LOCALAPPDATA%\ShaiWin\`。
- 每个 release 都附带 `SHA256SUMS.txt`，想校验下载完整性：

  ```powershell
  certutil -hashfile shaiwin-<版本>-win64.zip SHA256
  ```

- 双击以后托盘出现一张床单图标，屏幕顶端那根绳子就是它——拖一个窗口的标题栏到绳子上，松手。

**运行要求**：Windows 10 1809+ 或 Windows 11，x64。

> 维护者：`git tag v0.1.2 && git push origin v0.1.2` 就会触发 GitHub Actions，自动跑测试、构建、打包并创建对应的 release（见 `.github/workflows/release.yml`）；本地出同款包用 `scripts\package.ps1`。

### 方式二：自己搭建环境构建

```powershell
rustup default stable-x86_64-pc-windows-msvc   # 或 -gnu
cargo build -p shaiwin --release              # -> target\release\shaiwin.exe
cargo run -p shaiwin                          # 开发构建
```

**环境要求**

| 工具 | 版本 | 用途 |
| --- | --- | --- |
| Windows | 10 1809+ / 11，x64 | 运行 |
| Rust | stable 1.82+（edition 2021） | 全部 crate |
| MSVC 或 MinGW-w64 | VS 2022 Build Tools（C++ 桌面开发）/ WinLibs | 链接 |
| Windows SDK | 10.0.19041+ | `rc.exe`，把图标编进 exe |
| Visual Studio `cvtres.exe` | 随 MSVC 安装 | GNU 目标把 `.res` 转成 COFF；找不到也能构建，只是 exe 没有图标 |
| Node.js | 18+（可选） | 只在重新生成/校验图标和 README 用图时用 |
| PowerShell | 5.1（系统自带） | 端到端脚本 |

图标由 `crates/app/build.rs` 编进 exe，资源编译器都找不到时只打一行 warning，构建仍然成功。只改 Rust 代码时不需要 Node，也不需要资源编译器。CI 用的是 `windows-latest` + MSVC 工具链，本地用 GNU 工具链构建出来的代码完全一样。

### 验证

```powershell
cargo test --workspace                                                   # 82 个单元测试
cargo clippy --workspace --all-targets -- -D warnings
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-test.ps1
```

十个端到端脚本共用 `scripts/testkit.ps1`（Win32 声明、临时夹具窗口、独立 `LOCALAPPDATA`），跑的是真实窗口管理器，不碰你自己的窗口：

| 脚本 | 覆盖 |
| --- | --- |
| `smoke-test.ps1` | 挂起 → 渲染 → 强杀恢复 → 取下 |
| `hook-test.ps1` | 钩子、取消挂起、真鼠标拖拽上架 |
| `edge-test.ps1` | 重复挂起、外部恢复、最大化、程序退出 |
| `reveal-test.ps1` | 非常驻时的悬停、胶囊、展开/收起 |
| `zorder-test.ps1` | 卡片层 z 序、绳子热区、提示气泡 |
| `cards-test.ps1` | 多张卡片各占一格、拖动换位、可点区域、× 只摘卡片 |
| `position-test.ps1` | 六种情形下点卡片回原坐标 |
| `resize-test.ps1` | 侧边调宽、底边调高、重启后保持 |
| `settings-shot.ps1` | 设置窗口截图，并真的点快捷键框、点 ×、点保存 |
| `hang-latency.ps1` | 挂起链路分段计时 |

改完请跑上面三条命令。要动渲染或挂起判定，先在真机上手工验一遍，机器验不了的在说明里写清楚「还没验的」，不要写成做完了。

---

## 功能介绍

### 挂起与取下

| 操作 | 效果 |
| --- | --- |
| 把窗口顶边拖到绳子上（卡片本体不算，垂在绳子下面的那段不接） | 衣架展开、绳子变暖，松手挂起 |
| `Ctrl+2`（可改） | 挂起当前窗口 |
| `Ctrl+T`（可改） | 展开 / 收起晾衣架 |
| 鼠标停在屏幕最顶端（非常驻时） | 弹出展开气泡滑过或悬停时「展开晾衣架 · 卡片」 |
| 点击卡片 | 窗口长回原来的大小，**仍然挂着钩**；拖上架的回到屏幕正中，热键挂起的回原位置 |
| 右键卡片 | 取下 / 取消挂起 / 设置或清除标签 |

「取下」和「取消挂起」是两件事：前者只是把窗口放回桌面，之后它最小化还会自动回来；后者才是彻底不管这个窗口，而且**不碰它**——本来最小化的继续最小化。卡片上的 × 就是「取消挂起」。

衣架收起时，把窗口移到屏幕顶端不会点亮衣架，也不会把它挂上去；想用拖拽挂起，先让衣架展开。

### 晾衣架本身

- **沿绳拖动卡片**换位置，停在放手的地方；多张卡片各占一格，不会重叠。
- **拖卡片侧边 / 下边缘 / 底角**调宽 / 调高 / 同时调，重启后保持。
- 卡片多到一屏放不下时，按住卡片横向拖动。
- 衣架可以设成常驻，也可以只在鼠标贴到屏幕顶端时露头；非常驻时指针离开后 1 秒收起，空绳至少留 4 秒。
- 右键卡片 →「设置标签…」可以起个短标签，卡片顶部出现彩色标签带，颜色由文字决定；输入时卡片实时变化。
- 卡片层不常驻顶层：鼠标离开晾衣绳就沉到当前窗口下面，滑过或悬浮晾衣绳时置顶显示卡片窗口；
- 支持多显示器与 Per-Monitor V2 DPI，搬屏后按那块屏的 DPI 重算挂起区。

### 快照与外观

- 窗口快照走 `PrintWindow(PW_RENDERFULLCONTENT)` → 旧式 `PrintWindow` → 抓屏三级兜底。
- 抓不到画面时（Chrome / Electron / 部分游戏的空白客户区），显示**应用色渐变 + 大图标 + 标题**的占位卡，而不是一块白；崩溃后恢复出来的卡片也是这一张。
- 同一个应用的卡片同色（FNV-1a 哈希落到 12 色板）。
- 弧线飞行、弹簧落针、阵风摆动，落针不是硬切。
- 中英文界面与托盘菜单跟随系统语言。

### 资源与性能

- 文字改用系统 GDI 绘制，一屏卡片时私有内存约 11 MB（此前是 180 MB 上下）。
- 画布与 DIB 复用，飞行时取包围盒，按实测帧成本自动降帧。

### 稳定性

- **挂起、取下都不闪**：挂起是原地最小化，没有先回普通大小再消失的一步；点卡片时窗口直接到前台，任务栏不会因为程序自己而闪烁。
- **崩溃也能找回来**：挂起就是系统自己的最小化，晒窗被强杀或崩溃以后，任务栏按钮点一下窗口就回到挂起前的位置。挂起前还会先写日志，重启时用「窗口句柄 + 进程 ID + 进程创建时间 + 停靠矩形」四重校验，确认还是那个窗口才恢复成卡片。
- 取下失败有兜底：**判定看窗口最后真的落在哪儿**，对不上就用 `SetWindowPos` 补；仍失败则卡片飞回并提示原因，窗口不会失联。
- 管理员权限的窗口会被 UIPI 挡住，程序直接拒绝，不会假装成功。
- 保守过滤：只处理可见、有标题、非工具窗、非 cloaked、非最小化的窗口。
- 运行日志在 `%LOCALAPPDATA%\ShaiWin\shaiwin.log`，出问题先看这里。

### 托盘与设置

右键托盘图标：全部取下、展开/收起、设置、开机启动、退出。

托盘「设置」打开一个独立窗口：两个快捷键、晾衣架弧度、衣架常驻、拖到顶部挂起。

设置也写在 `%LOCALAPPDATA%\ShaiWin\settings.txt`，窗口没覆盖的项可以直接改文件。常用键：

| 键 | 作用 |
| --- | --- |
| `hang_mods` / `hang_vk` | 挂起当前窗口的快捷键 |
| `toggle_mods` / `toggle_vk` | 展开/收起的快捷键 |
| `sag` | 绳子弧度 0–16%，留空用默认 |
| `auto_hide_secs` | 非常驻时指针离开后多久收起，默认 `1`；`0` 只手动收起 |
| `always_on` | 衣架常驻 |
| `drag_hang` | 拖窗口到顶部挂起，`0` 可整个关掉 |
| `reduced_motion` | 减弱动画 |
| `excluded` | 按可执行文件名的排除名单 |
| `raise_known` | 点过气泡的「不再提示」就是 `1`；删掉这行重启会再弹 |

旧版留下的 `hover_open=`、`tags=`、`all_monitors=` 会被忽略，删掉那行即可。同目录还有 `parked.txt`（挂起日志，退出后自动清空）和 `shaiwin.log`。卸载时删掉整个 `%LOCALAPPDATA%\ShaiWin\` 就行。

---

## 待开发功能清单

### Windows

**v0.2 · 手感与日常可用**

- [ ] 滚轮滚动晾衣绳：卡片多到一屏放不下时，滚轮在绳子上横向滚动/翻页。现在只能按住卡片沿绳子拖，而卡片本身还兼着点击和抓边，多卡场景不够用
- [ ] 每个显示器一条独立的晾衣架（当前是「跟着最后一次挂起走」的单线）
- [ ] 卡片重排动画：插入/移除时邻居平滑让位
- [ ] 「减弱动态效果」跟随系统设置（`SPI_GETCLIENTAREAANIMATION`）
- [ ] 空线时的引导：绳上一张卡片都没有时该怎么挂上去，还没有话
- [ ] 自定义卡片特征信息




**v1.0 · 发布**

- [ ] 安装包（MSIX 或 Inno Setup）与卸载时恢复所有窗口
- [ ] `winget` / Scoop 清单
- [ ] 代码签名（未签名 exe 会被 SmartScreen 拦）
- [ ] 崩溃上报（默认关闭，需显式同意）
- [ ] 性能回归基准：空闲 CPU < 0.1%，挂 20 张卡片动画 60 fps

### macOS

`platform/macos/` 里已经放了一份**没有在 macOS 上编译过、也没运行过**的骨架（`Package.swift` + `CShaiWin` modulemap + 薄封装 + 最小 AppKit 入口）。它的作用只是让有 Mac 环境的贡献者不从零开始。下面把「还没做」和「可以直接复用」分开写清楚。

**macOS 上还没实现的**

| 部分 | 状态 | 说明 |
| --- | --- | --- |
| 窗口枚举 | 未实现 | 需要 `CGWindowListCopyWindowInfo` 列窗口、`AXUIElement` 拿标题与位置 |
| 窗口移动 / 停靠 | 未实现，且方案未验证 | `AXUIElementSetAttributeValue(kAXPosition)` 移到屏幕外会不会被系统拉回，Mission Control 会不会仍显示它，都要先在真机上试 |
| 停靠兜底方案 | 未实现 | 若上面的方案不成立，退到 `NSWindow.miniaturize()` 走系统最小化，卡片上放「恢复按钮 + 快照」，但失去「窗口真的消失」的体验 |
| 窗口快照 | 未实现 | `ScreenCaptureKit`，需要屏幕录制权限；没有权限时只能走占位卡 |
| 权限流程 | 未实现 | Accessibility / Screen Recording / Input Monitoring 三个权限的申请、引导与拒绝后的降级 UI |
| 菜单栏图标 | 只有骨架 | `NSStatusItem` 的菜单项、状态、i18n 都没接 |
| 全局快捷键 | 未实现 | `NSEvent.addGlobalMonitorForEvents` 或 `MASShortcut` |
| 渲染 | 未实现 | 计划 `NSView` + `CALayer`；现在只有一块透明面板和一条绳子 |
| 拖拽上架检测 | 未实现 | Windows 侧是轮询 + 系统移动循环判定，Mac 侧要重写 |
| 设置、日志、崩溃恢复 | 未实现 | `settings.txt`、`shaiwin.log`、四重校验恢复都还是 Windows 专属 |
| 多显示器 / Retina | 未实现 | 多屏坐标与 2x 位图适配 |
| 打包与签名 | 未实现 | `.app` 打包、公证（notarization）、`.icns` 图标 |

**macOS 可以直接复用的（`crates/core` 全部，经 `crates/ffi` 的 C ABI 调用）**

| 能力 | 复用方式 |
| --- | --- |
| 槽位布局、绳垂、最大数量 | `shaiwin_layout_*`，已导出 |
| 飞行弧线（起落点、路径） | `shaiwin_flight_*`，已导出 |
| 弹簧落针 | `shaiwin_core::spring` 已在 Rust 侧完整实现并有单测，但**尚未导出到 C ABI**，补一层 `#[no_mangle]` 包装即可 |
| 配色与设计尺寸 | 现在是 Rust 常量，Swift 侧先照抄同样的数字，后续补 `shaiwin_theme_*` |
| 命中测试、卡片索引 | `shaiwin_core::hit` 已有逻辑，`shaiwin_layout_index_at` 还需导出 |

结论：**手感相关的数学一行都不用重写**，Mac 外壳只负责 `NSWindow` / `AXUIElement` / 渲染 / 权限这四件事。构建方式（`cargo build -p shaiwin-ffi --release` 拿到 `libshaiwin.a`，再 `swift build` 链接）写在 `platform/macos/README.md` 里。

---

## 项目目录架构

```
ShaiWin/
├── .github/
│   └── workflows/
│       └── release.yml         打 tag 推送后自动测试、构建、打包并创建 release
├── Cargo.toml                  workspace：四个 crate，release 开了 LTO / strip / panic=abort
├── Cargo.lock
├── LICENSE                     MIT + Commons Clause
├── crates/
│   ├── core/                   纯 Rust，无系统依赖
│   │   ├── layout.rs           槽位布局、绳垂、最大数量、挂起区判定
│   │   ├── spring.rs           弹簧积分（超大 dt 也不炸）
│   │   ├── anim.rs             飞行弧线：起落点与路径
│   │   ├── theme.rs            配色（FNV-1a → 12 色板）与设计尺寸
│   │   ├── hit.rs              命中测试、卡片索引
│   │   └── lib.rs
│   ├── platform-win/           Win32 层，不碰 UI
│   │   ├── window.rs           窗口枚举、保守过滤、前台/恢复
│   │   ├── park.rs             原地最小化停靠（placement + 按实际矩形校验兜底）
│   │   ├── capture.rs          PrintWindow 三级快照 + 空白检测
│   │   ├── dib.rs / image.rs   DIB 读写、按哨兵色裁边
│   │   ├── icon.rs             ExtractIconEx + GetDIBits（含 1bpp 掩码兜底）
│   │   ├── monitor.rs          多显示器、Per-Monitor V2 DPI
│   │   └── lib.rs
│   ├── app/                    Windows 外壳：把上面两层接起来
│   │   ├── main.rs             入口、单实例互斥、消息循环
│   │   ├── line.rs             分层窗口（晾衣绳本体）与状态机
│   │   ├── render.rs           tiny-skia 渲染管线、卡片贴图缓存
│   │   ├── app.rs              挂起/取下编排、plan → 写日志 → execute
│   │   ├── worker.rs           后台线程：快照、图标提取
│   │   ├── journal.rs          挂起日志与崩溃恢复的四重校验
│   │   ├── tray.rs             托盘图标与菜单
│   │   ├── settings.rs         settings.txt 读写
│   │   ├── settings_ui.rs      设置窗口（五个自绘控件）
│   │   ├── tag_ui.rs           卡片标签编辑弹层
│   │   ├── text.rs             GDI 文字位图
│   │   ├── i18n.rs             中英文
│   │   ├── log.rs              shaiwin.log
│   │   ├── ico.rs              exe 图标资源
│   │   └── build.rs            把 .ico 编进 exe（windres / rc.exe + cvtres.exe）
│   └── ffi/                    C ABI，给未来 macOS 外壳复用 core
│       └── lib.rs
├── include/
│   └── shaiwin.h               上面这层的头文件
├── platform/
│   └── macos/                  未验证的 Swift/AppKit 骨架
│       ├── Package.swift
│       ├── README.md           分工、停靠方案、权限、还缺哪些导出
│       └── Sources/
│           ├── CShaiWin/module.modulemap
│           ├── ShaiWinCore/    对 C ABI 的薄封装
│           └── ShaiWin/        最小 AppKit 入口
├── scripts/
│   ├── testkit.ps1             端到端脚本共用的 Win32 声明与夹具窗口
│   ├── smoke-test.ps1 等 10 个 端到端测试
│   ├── package.ps1             本地打出与 release 相同的 zip
│   ├── build-icon.mjs          SVG → 九个尺寸 → assets/shaiwin.ico
│   ├── preview-icon.mjs        把九个尺寸拼成 assets/icon-preview.png 供肉眼检查
│   ├── check-exe-icon.mjs      核对 exe 里的图标资源与 .ico 是否一致
│   └── usage-shot.ps1 等       README 用图
├── assets/                     README 用图、图标、收款码
└── dist/                       本地产物
```

分层原则：`core` 不知道 Windows 存在，`platform-win` 不知道 UI 存在，`app` 只做编排。所以 macOS 外壳可以只换掉 `platform-win` 和 `app`，`core` 原样复用。

---

## 许可

本项目采用 **MIT License + Commons Clause** 授权，完整法律条款见 [LICENSE](LICENSE)。

**授权范围**

在保留版权声明与本许可声明的前提下，任何个人或组织均可免费取得本软件的副本，并有权使用、复制、修改、合并、发布、分发及再许可本软件，不受使用场景与部署规模的限制，包括：

- **商业与办公用途**：企业内部部署、日常办公使用、批量安装于任意数量的设备，均无需事先申请授权，亦不产生任何费用。
- **二次开发与再分发**：可修改源代码、变更名称、以闭源方式分发，或整体、部分地集成至自有项目。

**限制条款**

依据 Commons Clause，上述授权**不包括**向第三方销售本软件的权利。即不得将本软件或其衍生作品作为商品或服务**有偿**提供给第三方，包括但不限于：销售本软件；销售价值全部或主要来源于本软件功能的产品或服务；以收费方式提供与本软件相关的部署、托管、运维或定制开发服务。

如需取得上述销售授权，请在仓库提交 issue 说明用途。

> 说明：Commons Clause 不属于 OSI 认可的开源许可，本项目的授权模式为 source-available——源码照常公开，仅保留「不得销售」一项权利。代码托管平台可能将许可证标识显示为 `Other`。

---

## 请我喝杯咖啡

如果晒窗帮你少找了几次窗口，可以请我喝杯咖啡。

![请我喝杯咖啡](assets/donate.png)
