# zcode-switch 项目约定

## Git 规则（用户明确要求）

- **不要推送 git**：所有提交只保留在本地，未经用户明确要求不得执行 `git push`（无论 origin 指向哪个仓库）。
- 远程仓库：origin = https://github.com/wjsqqj/zcode-switch.git（fetch/pull/merge 可以，push 禁止）。

## 项目背景速记

- 本仓库是 ZCode（Electron 客户端）的账号切换/多实例管理器（Tauri 2 + 原生 Rust 后端）。
- 已实现 Profile 多开体系（src-tauri/src/profile.rs）：独立数据根 + ZCODE_DATA_BASE_DIR / ZCODE_DESKTOP_USER_DATA_DIR 注入 + PID 级进程管理 + 账号绑定唯一性。
- 关键逆向结论（/Applications/ZCode.app asar）：dataBaseDir 优先级 = setting.json > env ZCODE_DATA_BASE_DIR > HOME；凭据密钥派生自 homedir()，勿重定向 HOME/ZCODE_HOME。
- CLI 返回约定：run() 各分支统一 (String, i32) 元组，成功用 ok0()。
