# 内置 PDF 阅读与标注

纯 JavaScript 界面，使用固定版本的 PDF.js 6.3.289（Apache-2.0）和 pdf-lib 1.17.1（MIT）。渲染引擎、Worker、CMaps、标准字体、WASM 和许可证压缩后随 Desktop 打包，运行时无需 Node.js 或 CDN。

```sh
npm ci --ignore-scripts
npm test
npm run build
```

提交源码、锁文件及 `dist/`。`build.mjs` 使用固定版本的 pako 生成确定性的 gzip 资源，避免不同 Node.js 内置 zlib 版本导致差异；Desktop 的 `build.rs` 将它们嵌入二进制，CI 在 Linux 重建并检查资源是否与源码一致。PDF.js 的字体、CMaps、ICC 和 WASM 各自的许可证也在对应资源目录中。

`document.mjs` 保存页面顺序、旋转和标注的撤销历史。标注使用 PDF 页面坐标，`marks.mjs` 的同一绘制函数用于预览、截图和导出，避免缩放、CropBox 和旋转造成偏移。导出保留原页的文字及矢量内容，将新增标注作为透明 PNG 叠加到页面；导出文件中的这些标注已合并为页面内容，重开后可以继续添加新标注。当前不支持加密文档的编辑，以及原正文重排、数字签名和交互表单编辑。

Desktop 为每个窗口创建文件快照，仅通过带随机路径的 loopback HTTP 服务提供该文档和内置资源。截图和保存请求检查 Origin、大小与文件类型；保存位置通过系统文件对话框选择。截图独立存储到应用数据目录并通过现有消息及 Runner 路径进入 Codex／Claude，WebView 无通用文件系统接口。

macOS／Windows 使用现有 Wry 子 WebView；Linux 使用同一进程中的 GTK／WebKitGTK 窗口，支持 X11 和 Wayland。Linux 需安装 `libwebkit2gtk-4.1-0`，从源码构建需 `libwebkit2gtk-4.1-dev`。
