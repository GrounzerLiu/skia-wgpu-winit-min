# Skia + wgpu + winit 结合注意事项

记录把 Skia 的 Graphite/wgpu 后端嵌进 winit 窗口程序时踩到的问题。所有结论都来自
[这个仓库](https://github.com/GrounzerLiu/skia-wgpu-winit-min) 的实测：CI（windows-2022 源码构建）
与本机（Windows 11 / RTX 5060 / Vulkan 后端）都跑通过，离屏像素断言与窗口截图为准。

版本：`skia-safe` 0.158.0（Skia milestone 158）、`wgpu` 30.0.2、`winit` 0.31.0-beta.4、Rust 1.100。

---

## 1. 依赖怎么选

| 需求 | 选择 | 说明 |
|---|---|---|
| Skia 绑定 | `skia-safe = { version = "0.158", features = ["wgpu"] }` | `wgpu` 特性等价于 `wgpu-30`，即「Graphite 的 Dawn 后端跑在 wgpu 30 上」 |
| GPU | 不要自己再声明 `wgpu` | `skia_safe::wgpu` 就是它依赖的那个 wgpu，用它保证两边是同一个 `wgpu::Device` 类型 |
| 窗口 | `winit = "0.31.0-beta.4"` | beta，API 与 0.30 差异很大，见第 4 节 |
| `skia-wgpu` | 不要直接依赖 | 它是 shim，由 `skia-safe` 的 `wgpu` 特性带进来；应用只通过 `skia_safe::gpu::graphite::wgpu` 使用 |

如果应用自己另加一个不同大版本的 `wgpu`，`make_wgpu` 会因为类型不匹配直接编译不过；
这也是 `skia-safe` 把 `wgpu` 版本和 milestone 绑在一起的原因。

---

## 2. 为什么必须源码构建 Skia

`skia-safe` 的 `wgpu` 特性要求 `skia-bindings`：

1. 编译 Skia 的 Dawn 后端（GN 参数 `skia_use_dawn` 等）；
2. 用 Dawn 的生成器产出 `webgpu.h` / `webgpu_cpp.h`，供 C++ 侧和 bindgen 共用同一份结构体、枚举与函数签名；
3. Skia 里的 `wgpu*` 符号保持未定义，链接时由 `skia-wgpu` 用 Rust 实现补上。

这套变体**没有预编译二进制**。`skia-binaries` 的 release 里有 `-graphite-...` 系列，但没有 `-wgpu`
（实测下载返回 404）。原因是 rust-skia 的 `windows-binaries.yaml` 矩阵里根本没有 `wgpu` 组合，
带 `wgpu` 的只有 QA 流程（all-features），产物是 Actions artifact、按整组特性算 key、90 天过期，
不会发到 release 页。

所以第一次 `cargo build` 会自动：下载 Skia 源码（`codeload.github.com/rust-skia/skia/tar.gz/m158.4`）
→ 下载 Dawn 与生成器的 Python 包（`jinja2`、`markupsafe`）→ GN + Ninja 编译 → bindgen。

### 不要设 `SKIA_SOURCE_DIR`

设了这个变量，`skia-bindings` 会进入 **offline 构建**，而 offline 明确拒绝 `wgpu`：

```
panic: Offline builds do not support the `wgpu` feature, because it downloads Dawn
and the Python packages of Dawn's generator.
```

也就是说 wgpu 这条路必须联网。想「把源码放干净目录」的想法在这里行不通。

### 构建耗时（实测）

| 机器 | 耗时 | 说明 |
|---|---|---|
| GitHub `windows-2022`（4 核） | 约 8 分钟 | 仅 Skia 编译；整 job 12 分半（含依赖与测试） |
| 本机 32 核 | 3 分 56 秒 | 含全部依赖、Skia、`skia-safe` 与示例 |

之后增量编译只要几秒——`target/` 里已经有编好的 Skia 静态库。

---

## 3. Windows 上的构建前置条件与坑

必需：**MSVC**、**LLVM（clang-cl）**、**Ninja**、**Python 3**。

`skia-bindings` 找 LLVM 的顺序：环境变量 `LLVM_HOME` → `C:\Program Files\LLVM` → `C:\LLVM`
→ `%USERPROFILE%\scoop\apps\llvm\current`，并且要求该目录下同时存在
`bin\clang-cl.exe` 和 `lib\clang\<version>\`。找不到就直接 panic：

```
panic: Unable to locate LLVM installation
```

用 winget 装到默认路径最省事（`winget install --id LLVM.LLVM -e`），不需要设环境变量。

### 坑 1：cargo registry 目录不可写

错误形态：`os error 5 拒绝访问`，发生在 `...\registry\src\...\skia-bindings-0.158.0\skia-m158.4`。

原因：构建脚本把 Skia 解包到 **crate 自己的源码目录**（`std::env::current_dir()`），而这个目录在
cargo registry 里。如果该位置不可写（只读挂载、沙箱、受限令牌等），就会失败。

绕过：把 `CARGO_HOME` 指到可写位置：

```powershell
$env:CARGO_HOME = "D:\path\to\writable\.cargo-home"
cargo build --release
```

### 坑 2：Skia 源码里的符号链接（os error 1314）

Skia 的 tarball 里有符号链接，例如 `src/ports/fontations/Cargo.toml -> ../../../bazel/external/fontations/Cargo.toml`。
Windows 上创建符号链接需要 `SeCreateSymbolicLinkPrivilege`，普通用户只有在**开发者模式**开启时才允许；
受限令牌（沙箱）下即使开了开发者模式也会报：

```
客户端没有所需的特权。(os error 1314) when symlinking ...
```

处理方式（按优先级）：

1. 开启 Windows 开发者模式（设置 → 隐私和安全性 → 开发者选项），用自己的终端编译；
2. 用管理员终端编译；
3. 或者**预先解包**：`skia-bindings` 在 `repo_name`（就是 `skia`）目录非空时会跳过下载与解包，
   所以可以先自己解：

   ```bash
   # crate 目录：<CARGO_HOME>/registry/src/<index>/skia-bindings-0.158.0/
   mkdir -p skia
   curl -L -o skia.tar.gz https://codeload.github.com/rust-skia/skia/tar.gz/m158.4
   tar -xzf skia.tar.gz -C skia --strip-components=1
   ```

   注意解到 `skia/` 而不是 `skia-m158.4/`：代码里判断「已下载」用的是 `repo_name`（`skia`），
   `skia-m158.4` 只是临时解包目录，会被删掉重建。

### 坑 3：Python 版本

Skia 的 GN 脚本对最新 Python 没测过。本机 Python 3.14 可以通过，但如果遇到 python 相关报错，
把 3.12/3.11 放到 PATH 前面再试。

---

## 4. winit 0.31 beta 的 API 迁移

官方 `skia-safe/examples/wgpu-window` 用的是 winit **0.30**，照抄到 0.31 会编译失败。差异：

| 0.30 | 0.31.0-beta.4 |
|---|---|
| `Window` 是具体类型 | `Window` 是 trait，窗口是 `Box<dyn Window>` |
| `Window::default_attributes()` | `WindowAttributes::default()` |
| `resumed(&mut self, ...)` 里建窗口 | 新增 `can_create_surfaces(&mut self, &dyn ActiveEventLoop)`，建窗口/建 surface 放这里 |
| `&ActiveEventLoop` | `&dyn ActiveEventLoop`（trait object） |
| `event_loop.run_app(&mut app)` | `event_loop.run_app(app)`，按值且要求 `'static` |
| `WindowEvent::Resized` | `WindowEvent::SurfaceResized` |
| `window.inner_size()` | `window.surface_size()` |

最短骨架：

```rust
impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        let window = event_loop
            .create_window(WindowAttributes::default().with_title("Skia on wgpu"))
            .expect("creating the window failed"); // Box<dyn Window>
        // ... 建 wgpu surface / device / Skia 上下文
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::SurfaceResized(size) => { /* 重新 configure */ }
            WindowEvent::RedrawRequested => { /* 画一帧 */ }
            _ => {}
        }
    }
}
```

### 怎么拿到 `'static` 的 `wgpu::Surface`

`Instance::create_surface` 接受实现了 `HasWindowHandle + HasDisplayHandle` 的值，返回值借用它。
窗口现在是 trait object，直接传 `&dyn Window` 会得到带借用生命周期的 `Surface<'_>`，
存进结构体很别扭。包一层自有类型即可：

```rust
struct OwnedWindow(Arc<dyn Window>);

impl HasWindowHandle for OwnedWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> { self.0.window_handle() }
}
impl HasDisplayHandle for OwnedWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> { self.0.display_handle() }
}

// Arc<dyn Window> 是 'static，所以这里拿到 Surface<'static>
let surface = instance.create_surface(OwnedWindow(window.clone()))?;
```

winit 本身已经给 `dyn Window` 实现了这两个 trait，所以包装类型只是把生命周期「固定」住。

---

## 5. wgpu 30 相对旧版的 API 变化

| 旧写法 | wgpu 30 |
|---|---|
| `request_adapter(...)` 返回 `Option<Adapter>` | 返回 `Result<Adapter, RequestAdapterError>` |
| `request_device(&desc, None)` | `request_device(&DeviceDescriptor::default())`，一个参数，返回 `(Device, Queue)` |
| `surface.get_current_texture()?` | 返回枚举 `CurrentSurfaceTexture`，需要 match |
| `frame.present()` | `queue.present(frame)` |
| `RenderPassDescriptor { label, color_attachments, depth_stencil_attachment, timestamp_writes, occlusion_query_set }` | 多了 `multiview_mask: Option<NonZeroU32>` |
| `RenderPassColorAttachment { view, resolve_target, ops }` | 多了 `depth_slice: Option<u32>` |

`CurrentSurfaceTexture` 的处理要点：`Success`/`Suboptimal` 直接画；`Outdated`/`Lost` 要
`surface.configure` 后跳过本帧；`Timeout`/`Occluded` 直接跳过；`Validation` 说明有校验错误。

---

## 6. Skia Graphite/wgpu 的调用顺序与生命周期

```rust
// 初始化一次
let mut context = graphite::wgpu::context_factory::make_wgpu(&device, &queue, None)?;
let mut recorder = context.make_recorder(None)?;

// 每帧
let mut surface = graphite::wgpu::surfaces::wrap_texture(
    &mut recorder, &texture, ColorType::BGRA8888, None, None,
)?;                       // 注意 let mut：canvas() 取 &mut self
paint_frame(surface.canvas(), w, h);
drop(surface);            // 必须在 snap 之前显式释放
let mut recording = recorder.snap()?;
context.insert_recording(&graphite::InsertRecordingInfo::new(&mut recording));
context.submit(None);
```

四条硬性约束：

1. **`wrap_texture` 的 surface 要先 drop，再 `snap()`**。`Recorder::snap` 有断言：
   `"must not be called while an ImageProvider runs inside one of its draws"`，官方驱动也是先
   `drop(surface)` 再 snap。
2. **析构顺序**：`Recorder` 与 `Context` 必须比 `Device` 先析构。Rust 按字段声明顺序 drop，
   所以结构体里要把 `skia` 字段写在 `device` 前面。
3. **`InsertRecordingInfo` 要 `&mut Recording`**：`insert_recording` 会通过内部指针回写
   failure 结果、释放 volatile lazy proxy。
4. **Skia 与 wgpu 共用同一个 queue**，`submit` 之后同一队列里后续的 wgpu 命令自然能看到 Skia 的绘制，
   不需要额外同步（这也是这套方案最大的好处）。

### Graphite 只画 GPU 上的图像

`canvas.draw_image` 对 raster 图像无效，必须先上传：

```rust
let image = graphite::images::texture_from_image(&mut recorder, &raster_image)?;
canvas.draw_image(&image, (x, y), None);
```

反过来，把应用自己的 `wgpu::Texture` 当图像画，用
`graphite::wgpu::images::wrap_texture(&mut recorder, &texture, ColorType, AlphaType, None)?`
（纹理需要 `TEXTURE_BINDING`，并且要保证在 Graphite 提交前已经写好内容）。

---

## 7. 颜色空间与格式匹配

- `wrap_texture` 的 `ColorType` 必须和纹理的真实格式一致：
  `Bgra8Unorm`/`Bgra8UnormSrgb` → `ColorType::BGRA8888`，其余 → `RGBA8888`。
- **优先选非 sRGB 的交换链格式**。Skia 写进纹理的是已经 sRGB 编码的值，如果交换链是
  `*Srgb` 格式，硬件会再编码一次，颜色会偏。
  ```rust
  let caps = surface.get_capabilities(&adapter);
  if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) { config.format = *f; }
  ```
- 交换链纹理天然有 `RENDER_ATTACHMENT`，可以直接被 `wrap_texture` 包成 Skia surface。

---

## 8. 帧循环

- 连续动画：在 `RedrawRequested` 里画完再 `window.request_redraw()`。
- 每帧对当前交换链纹理新建一个 Skia surface 是可以的（官方驱动对每张纹理也是新建 + drop）。
- 窗口尺寸变化用 `SurfaceResized` 里的 `size` 重新 `configure`；不要忘了宽高为 0 时跳过
  （最小化时会出现）。

---

## 9. 没有显示器时怎么验证

CI 里开不了窗口，但可以把 Skia 画进离屏纹理再读回像素断言：

```rust
let texture = device.create_texture(&wgpu::TextureDescriptor {
    label: Some("skia target"),
    size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
    mip_level_count: 1,
    sample_count: 1,
    dimension: wgpu::TextureDimension::D2,
    format: wgpu::TextureFormat::Rgba8Unorm,
    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::COPY_SRC,
    view_formats: &[],
});
// ... 用 Skia 画，submit_and_wait()
encoder.copy_texture_to_buffer(texture.as_image_copy(), /* buffer */, texture.size());
buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None })?;
let pixels = buffer.get_mapped_range(..)?;
```

细节：`copy_texture_to_buffer` 的 `bytes_per_row` 必须是 256 的倍数，纹理宽度选 64 就刚好
（`64 * 4 = 256`）。`submit_and_wait()` 比 `submit()` 方便，省掉自己等 GPU。

windows-latest runner 上 `request_adapter` 会拿到 DX12 的 WARP 软件适配器，断言同样通过；
本机则跑在真实 GPU（本机实测是 NVIDIA RTX 5060 + Vulkan 后端）。

---

## 10. 参考资料

- rust-skia 仓库：<https://github.com/rust-skia/rust-skia>
- 官方示例 `skia-safe/examples/wgpu-window`（旋转立方体，winit 0.30）
- 官方驱动 `skia-org/src/drivers/wgpu.rs`（离屏渲染 + 读回的写法）
- 集成测试 `skia-safe/tests/wgpu.rs`（像素断言）
- 架构说明 `docs/wgpu-backend-architecture.md`、ADR 0003 / 0004
- `skia-wgpu/FEATURES.md`：shim 向 Skia 报告哪些 WebGPU 特性
- 本仓库的示例：`src/main.rs`（开窗）、`src/bin/offscreen-check.rs`（离屏断言）
