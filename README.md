# Magic Pocket

Magic Pocket 是一个基于 `Tauri + Rust + Vite + React` 的轻量桌面剪贴板工具，整体是小窗、毛玻璃、接近 macOS 工具应用的风格。

## 功能

- 自动监听系统剪贴板
- 保存最近历史记录
- 支持文本、图片、文件三类内容
- 全文搜索和关键词高亮
- 收藏常用内容
- 单击列表项直接回贴到剪贴板
- 全局快捷键唤起：`Ctrl + Shift + Space`

## 使用方式

启动应用后会持续监听剪贴板。

- 单击任意记录：复制该记录回剪贴板
- 搜索框：按内容、文件名、标签筛选
- 收藏：置顶常用记录
- 删除：移除当前记录
- 右上角按钮：隐藏应用 / 退出应用

## 本地开发

需要先安装：

- Node.js 18+
- Rust
- Tauri 2 的 Windows 构建依赖

安装依赖：

```bash
npm install
```

启动开发环境：

```bash
npm run tauri:dev
```

## 打包发布

生成 Windows 安装包：

```bash
npm run tauri:build
```

构建产物默认输出到：

```text
src-tauri/target/release/bundle/nsis/
```

## GitHub 发布建议

如果你准备让其他电脑直接下载使用，建议把源码推到 GitHub 后，在 `Releases` 页面上传 `NSIS` 安装包，而不是让普通用户直接从源码运行。

建议发布内容：

- 源码仓库
- Windows 安装包
- 简短更新说明

## 技术栈

- Tauri 2
- Rust
- React 18
- Vite
