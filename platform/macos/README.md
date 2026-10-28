# macOS 外壳（未验证的骨架）

> 这个目录里的代码**没有在 macOS 上编译或运行过**。它的作用是让有 Mac 环境的贡献者不用从零开始：核心手感（布局、弹簧、飞行弧线、配色）已经在 `shaiwin-core` 里，通过 `shaiwin-ffi` 的 C ABI 暴露出来。

## 为什么 macOS 版值得做，但优先级低

macOS 已经有 Stage Manager 和多桌面：把窗口「收起来」这件事系统本身做得不错。晒窗在 mac 上的价值主要在于**「同一应用的多窗口分组」与「晾衣架的视觉隐喻」**，而不是功能刚需。所以先做 Windows，mac 外壳等社区补齐。

## 分工

| 部分 | 谁负责 |
| --- | --- |
| 布局（槽位、绳垂、最大数量） | `shaiwin-ffi` → `shaiwin_layout_*` |
| 弹簧与飞行弧线 | `shaiwin-ffi` → `shaiwin_flight_*`（弹簧接口尚未导出，见下） |
| 颜色与设计尺寸 | 目前是 Rust 常量；Swift 侧先写死同样的数字，后续补 `shaiwin_theme_*` |
| 窗口枚举、移动、快照 | Swift：`CGWindowListCopyWindowInfo`、`AXUIElement`、`ScreenCaptureKit` |
| 渲染 | Swift：`NSView` + `CALayer`，或 Metal |
| 托盘 / 快捷键 / 权限 | Swift：`NSStatusItem`、`NSEvent`、`AXIsProcessTrusted` |

## 构建

```bash
# 1. 构建静态库（在仓库根目录）
cargo build -p shaiwin-ffi --release
# 产物：target/release/libshaiwin.a

# 2. 构建 Swift 外壳
cd platform/macos
swift build \
  -Xlinker -L../../target/release \
  -Xlinker -lshaiwin
```

`Sources/CShaiWin/module.modulemap` 通过相对路径引用仓库根目录的 `include/shaiwin.h`，所以不需要拷贝头文件。

## 核心问题：macOS 上「停靠」怎么做

Windows 的 `SetWindowPos` 可以把窗口丢到虚拟桌面外。macOS 没有等价物，候选方案：

### 方案 A：AXUIElement 移出屏幕（首选，需 Accessibility 权限）

```swift
let app = AXUIElementCreateApplication(pid)
AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString,
                             AXValueCreate(.cgPoint, &farOffscreenPoint)!)
```

**待验证**：系统是否会把窗口拉回可视区；Mission Control 是否仍然显示它；恢复时 `kAXPosition` + `kAXSize` 是否一次性生效。

### 方案 B：移到一个虚拟的「屏幕外」坐标 + 隐藏

`NSWindow.setFrameOrigin` 同样可能被 AppKit 约束。可以尝试 `window.collectionBehavior.insert(.stationary)` 配合。

### 方案 C：私有 API `CGSSetWindowTransform`

能真正做到「缩小并移走」，但不可上架 App Store，也不保证未来版本可用。

### 方案 D：不真的移走，改为系统级收纳

用 `NSWindow.miniaturize()` 把窗口最小化到 Dock，晾衣架上放的是「恢复按钮 + 快照」。这样不违反系统约束，但失去「窗口真的消失、桌上干净了」的核心体验。

**建议**：先验证 A；A 不成立就退到 D，并把 D 做成可配置项（「macOS 上使用系统最小化」）。

## 权限

| 权限 | 用途 | 获取方式 |
| --- | --- | --- |
| Accessibility | 枚举/移动其他应用的窗口 | 系统设置 → 隐私与安全性 → 辅助功能 |
| Screen Recording | 抓取窗口快照（ScreenCaptureKit） | 系统设置 → 隐私与安全性 → 屏幕录制 |
| Input Monitoring | 全局快捷键（如果不用 `NSEvent.addGlobalMonitorForEvents` 的按键部分） | 系统设置 → 隐私与安全性 → 输入监控 |

三个权限都需要用户手动开启，且首次请求时系统会重启应用。UI 上必须解释清楚为什么需要。

## 还需要从 Rust 导出的接口

* `shaiwin_spring_new(x, response, damping)` / `shaiwin_spring_step` / `shaiwin_spring_value`
* `shaiwin_theme_card_size()` 等设计常量
* `shaiwin_layout_index_at(x, count)`

这些都在 `shaiwin-core` 里，补 `#[no_mangle]` 包装即可，不需要重写逻辑。

## 目录

```
platform/macos/
├── Package.swift
├── README.md
└── Sources/
    ├── CShaiWin/module.modulemap     把 include/shaiwin.h 暴露给 Swift
    ├── ShaiWinCore/ShaiWinCore.swift 对 C ABI 的薄封装
    └── ShaiWin/main.swift            最小 AppKit 骨架（托盘 + 透明面板 + 绳子）
```
