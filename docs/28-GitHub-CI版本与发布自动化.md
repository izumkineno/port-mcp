# 28 GitHub CI、版本与发布自动化

本文档说明 Rust `port-mcp` 的 GitHub Actions CI、自动版本更替和 GitHub Release 发布口径。`mcp-server/` 是独立的 TypeScript workflow server，不参与本发布链路。

## 工作流总览

| Workflow | 文件 | 触发 | 权限 | 职责 |
| --- | --- | --- | --- | --- |
| Rust CI | `.github/workflows/ci.yml` | PR 与普通 push | `contents: read` | 运行 Rust 格式、编译和测试门禁。 |
| Version Tag | `.github/workflows/version.yml` | 默认分支 push（当前为 `master`） | `contents: write` | 根据 Conventional Commits 计算下一版本，更新 Cargo 版本并推送 `vX.Y.Z` tag。 |
| Release | `.github/workflows/release.yml` | `v*` tag push | gate/build 为 `contents: read`，publish 为 `contents: write` | 重新运行 Rust gate，校验 SemVer tag 与默认分支祖先关系，构建多平台二进制并创建 GitHub Release。 |

所有 workflow 都从仓库根目录运行 Rust 命令，不安装 Node，不运行 npm，也不打包 `mcp-server/`。

## CI 门禁

Rust CI 与 Release workflow 的发布前 gate 均包含：

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
```

这些检查默认覆盖无硬件路径。真实串口与 VISA 设备验收仍按 acceptance 文档和本地手工验证执行，不进入 GitHub Actions 强制门禁。

Ubuntu runner 在运行这些 Rust gate 或构建 Linux release artifact 前会安装 `pkg-config` 和 `libudev-dev`。这是 `serialport` 依赖链中的 `libudev-sys` 在 Linux 上定位 `libudev.pc` 所必需的系统依赖。

## 版本规则

版本源是根目录 `Cargo.toml` 的 `[package].version`。`version.yml` 会逐个读取 latest `vX.Y.Z` tag 之后的提交信息，并按提交 subject 和 breaking footer 选择最高优先级 bump：

| Commit 形式 | 版本变化 |
| --- | --- |
| `type!:` 或 footer `BREAKING CHANGE:` / `BREAKING-CHANGE:` | major |
| `feat:` / `feat(scope):` | minor |
| `fix:`、`perf:`、`refactor:` 及其 scoped 形式 | patch |
| `docs:`、`test:`、`ci:`、`chore:`、`chore(release):` | 默认不发布 |

如果没有 release-worthy commit，workflow 成功退出，不提交版本变更、不创建 tag、不发布 Release。

## 自动回写与防循环

`version.yml` 会在默认分支（当前为 `master`）上串行执行，并使用 `contents: write` 权限执行以下动作：

1. 运行 Rust gate。
2. 计算下一版本。
3. 更新 `Cargo.toml`，运行 Cargo 重新同步 `Cargo.lock`，并用 `--locked` 复核。
4. 提交 `chore(release): vX.Y.Z [skip ci]`。
5. 创建并推送 `vX.Y.Z` tag。

防循环策略包括：

- `concurrency` 串行化默认分支发布计算。
- 跳过 `github-actions[bot]` 触发的版本提交。
- 跳过包含 `[skip ci]` 或以 `chore(release):` 开头的提交。
- 推 tag 前显式检查目标 tag 不存在。

如果版本提交成功但 Release 构建失败，可以在 GitHub Actions 中重新运行 tag 对应的 Release workflow。tag 是发布恢复边界。

## Release 产物

`release.yml` 只由 `v*` tag 触发，并在运行时要求 tag 严格匹配 `^v\d+\.\d+\.\d+$` 且 tag commit 是仓库默认分支的祖先。每个 tag 会构建并上传以下产物：

| Target | Runner | Archive |
| --- | --- | --- |
| `x86_64-pc-windows-msvc` | `windows-latest` | `port-mcp-vX.Y.Z-x86_64-pc-windows-msvc.zip` |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `port-mcp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-apple-darwin` | `macos-14` | `port-mcp-vX.Y.Z-aarch64-apple-darwin.tar.gz` |

每个 archive 只包含编译后的 Rust 二进制，并额外上传同名 `.sha256` 校验文件。GitHub Release 使用 `port-mcp vX.Y.Z` 作为标题，并由 GitHub 生成 release notes。

## GitHub 权限要求

仓库需要允许 GitHub Actions 使用 `GITHUB_TOKEN` 写入 contents，才能让 `version.yml` 推送版本提交、tag 和 Release。如果默认分支启用了分支保护，需要确认 GitHub Actions bot 是否允许写入，或改用受控 PAT 并更新 workflow token 配置。

若启用 tag protection，也需要允许 workflow 创建 `v*` tag。

## 本地验证边界

本地可以验证：

```powershell
./scripts/next-version.ps1 -CurrentVersion 0.1.0 -CommitText "feat: add release automation"
cargo fmt --check
cargo check --locked
cargo test --locked
```

实际推送版本提交、创建 tag、上传 Release artifact 需要在 GitHub Actions 环境中验证。首次启用后建议手工检查一次：版本提交是否落在默认分支、tag 是否指向版本提交、四个平台 artifact 是否完整上传、Release notes 是否符合预期。
