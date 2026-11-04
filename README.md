# sfxer

本地音效素材库管理器：把散落在各个文件夹里的音效收进一个可搜索的库，用标签、备注和显示名组织，不移动也不改写原始文件。

Windows 10 / 11 x64，Tauri 2 + Rust，单文件约 8 MB。

## 数据说明

标签、备注、显示名、评分、波形峰值全部写在本机 SQLite 库里（`%APPDATA%\dev.zara.sfxer\sfx.db`）。程序只读取音频文件，从不写入、改名、移动或删除它们；从库中「移除」也只是删除数据库里的条目。

## 功能

- 递归扫描添加的文件夹，任意深度；跳过 `node_modules`、`$RECYCLE.BIN`，`.` 与 `_` 开头的目录可在设置里选择是否包含
- 支持扩展名：mp3 / wav / w64 / flac / ogg / opus / m4a / aac / aiff / caf / ape / mpc / wv / tta / mka / mkv / webm / dsf / 3gp / amr / ac3 等
- 搜索名称、显示名、标签与备注；按格式、时长、采样率、声道、评分星级筛选
- 详情栏五星评分，只写数据库，不碰文件元数据
- 波形解码与峰值分析，可按峰值 dB 排序；APE 与 WAV 内封装 MP3 也能播放和生成波形
- 批量打标、批量备注、整理模式（`T`）逐条连续打标
- 底部播放条：波形拖动定位、音量、顺序播放 / 列表循环 / 单曲循环，当前筛选列表即播放队列
- 导入时显示查找与解析进度、当前文件和百分比，可随时取消；结束后给出新增 / 更新 / 移除 / 失败统计
- 来源文件夹在设置 → 导入与扫描里管理；标签、备注与评分可导出为 JSON，换机后导入合并

## 构建

```bash
npm install
cd src-tauri && cargo build --release   # 产物 src-tauri/target/release/sfxer.exe
npx tauri build                          # 另需联网下载 NSIS 工具，产出安装包
node tools/make-test-audio.mjs          # 生成 test-library/ 样例音频（不入库）
cargo test --lib                         # 18 项后端测试，依赖上一步生成的样例音频
```

## 快捷键

| 键 | 作用 |
| --- | --- |
| `空格` | 播放 / 暂停 |
| `↑` `↓` | 在列表中移动 |
| `Alt+←` `Alt+→` | 上一首 / 下一首 |
| `L` | 播放方式：只播一次 → 列表循环 → 单曲循环 |
| `F2` | 修改显示名 |
| `T` | 整理模式 |
| `Ctrl+F` | 跳到搜索框 |
| `Ctrl+A` | 全选当前筛选结果 |
| `Ctrl+I` `Ctrl+B` | 显示栏 / 侧栏开关 |
| `Delete` | 从库中移除条目（磁盘文件保留） |

## 更新

设置 → 更新：显示当前版本，可开关「自动检查更新」（每次启动匿名请求一次 GitHub Releases API，不需要密钥，不发送任何库内容），仓库名是链接，点击用系统浏览器打开；也可以手动点「检查更新」。发现新版本会弹窗显示版本、更新说明和下载入口，每个版本只提示一次。

图标来自 [Fluent System Icons](https://github.com/microsoft/fluentui-system-icons)（MIT）。
