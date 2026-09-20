<p align="center">
  <img src="apps/desktop/src-tauri/icons/128x128@2x.png" alt="MOGA" width="112" height="112">
</p>

<h1 align="center">Make Overleaf Great Again</h1>

<p align="center"><strong>Overleaf Account Switcher · Overleaf 现代工作流</strong></p>
<p align="center">连通 agent，无感换号项目迁移，让大型论文不再困于编译超时。</p>
<p align="center"><strong>简体中文</strong> · <a href="README.en.md">English</a></p>

<p align="center">
  <a href="#development"><img src="https://img.shields.io/badge/Rust-Tauri_2-222222?style=flat-square&amp;logo=rust" alt="Rust + Tauri 2"></a>
  <a href="#download"><img src="https://img.shields.io/badge/Windows-x64-0078D4?style=flat-square" alt="Windows x64"></a>
  <a href="#download"><img src="https://img.shields.io/badge/macOS-arm64_%2F_x64-555555?style=flat-square&amp;logo=apple" alt="macOS arm64 / x64"></a>
  <a href="#docker"><img src="https://img.shields.io/badge/Docker-amd64_%2F_arm64-2496ED?style=flat-square&amp;logo=docker&amp;logoColor=white" alt="Docker amd64 / arm64"></a>
</p>

<p align="center">
  <a href="#about">背景</a> · <a href="#features">功能概览</a> · <a href="#architecture">工作原理</a> ·
  <a href="#download">下载</a> · <a href="#quick-start">快速开始</a> ·
  <a href="#skills">Overleaf Skills</a> · <a href="#docker">Docker</a> · <a href="#structure">项目结构</a>
</p>

<a id="about"></a>

## 当 LaTeX 变得简单

随着LLM 不断发展，LaTeX 排版和编译的技术门槛正在降低。agent 已经能帮我们配置 TeX 环境、修改论文、排查报错，再把源码编译成 PDF。很多时候，你只需要说明想做什么；在获得授权后，agent 还能直接处理本地的文献、实验数据和图片，不必先把材料一份份上传。

而在这之前，Overleaf 曾是很多人的不二之选：

- **不用先配环境。** 不必从安装 TeX Live、MiKTeX 开始，再逐一处理编译引擎、宏包、字体与模板的兼容问题。
- **编译不再全靠自己的电脑。** 编译吃资源，云端处理能减轻本机负担，对配置有限的设备尤其方便。
- **模板与多人实时协作。** 打开现成模板就能写，和合作者在同一个项目里修改、讨论，不用来回传文件、猜哪个才是最终版。

这些优点今天仍然成立。只不过写作流程已经不只在浏览器里了：agent 可以按项目需要配置 pdfLaTeX、XeLaTeX 或 LuaLaTeX，用 latexmk 完成编译，还能结合本地材料直接修改文章。**写 LaTeX 越来越方便，Overleaf 也应该接上这套现代工作流。**

虽然 Overleaf 也有自己的 AI 功能，但当文献、数据和工具都在本地时，来回上传材料、搬运上下文和同步修改，还是很麻烦。Free 计划的编译时限又会让大型项目卡在“编译超时”；想换到另一个有合适权益的账号，项目迁移、协作权限和 agent 凭据也得重新处理。

尽管agent能够很轻易地完成一套latex写作的全流程，但是all in agent离现实还有一定的距离，尤其是一些科研论文的撰写上，最终还是要回到人的手上修改，正是因为如此，overleaf随时 **“可以手动微调改，即时编译，即时看到PDF产物”** 的作用还在，我们真正需要的是一个能在agent撰写和人类接手之间从容转换的工具或桥梁，在overleaf的基础上接入现代的工具流并弥补其大部分不好的地方就足够使其成为这样的工具，让overleaf再次伟大。 **Overleaf Account Switcher 是一个本地优先的 Overleaf 账号与项目工作台。** 通过 [overleaf-skills](https://github.com/YuanzAAi/overleaf-skills)，让 Codex、Claude Code 处理本地材料、编辑 Overleaf 项目、调用云端编译并取回 PDF；需要换号时，把项目迁移、浏览器会话与 skills 凭据一起衔接起来。云端协作继续用，agent 也能直接参与，不必再手动来回搬家。

<a id="features"></a>

## 功能概览

| 模块 | 功能 |
| --- | --- |
| 账号管理 | 账号卡片与列表、搜索筛选、批量选择、别名维护、账号密码登录、Cookie 导入、文件导入队列、单文件与多文件导出。 |
| 登录态与凭据 | Cookie 身份核验与密码恢复，Git token 获取与生成，本地密码更新、远端改密，凭据复制与状态检查。 |
| 无感换号 | 在当前 Chrome 档案中切换账号，校验目标身份，刷新订阅信息；按需联动项目迁移与 skills 凭据同步。 |
| 项目管理 | 迁移预览、自有项目复制、共享项目重新加入、目标可访问性核验；支持远端项目清理与本地账号移除。 |
| Overleaf Skills | 安装、更新与卸载 Codex / Claude Code skills，共用项目读写、Git 历史、云端编译和 PDF / 源码 / 日志下载能力。 |
| 注册与订阅 | 注册流程、reCAPTCHA 与邮箱验证、地址和支付信息填写、试用资格检测、计划切换、额外试用与取消续订。 |
| 卡片与地址 | 卡片批量添加、状态标记、选择导出与批量删除，地址获取、选择与复制。 |
| 任务与运行 | 并发队列、账号任务锁、等待输入、取消与重试、实时日志、代理设置、版本更新、浏览器档案与临时文件清理。 |

### 为什么选择 Overleaf Account Switcher

- **让 agent 直接参与论文工作。** 本地材料、Overleaf 项目和云端 PDF 串成一条流程，减少反复上传与复制粘贴。
- **把换号和项目迁移连起来。** 需要更换有专业版权益的账号时，先处理项目归属与访问权限，再继续写作。
- **浏览器与 skills 使用同一套账号。** 无感换号时按需同步 Cookie 和 Git token，不用分别手动配置。
- **Cookie 失效后继续处理。** 已保存密码的账号可尝试重新登录恢复，再继续原任务；遇到 reCAPTCHA 或邮箱验证时需等待用户处理。
- **编译时长不再烦恼。** 当你有多个专业版权益的账号的时候，你通过无感换号和项目迁移，能够很好地无缝衔接下一个专业版的账号，从而避免因free号的编译时长受限。

编译时限通常取决于**项目所有者**的计划，仅切换协作者账号不会增加时限。Overleaf Account Switcher 不改变计划权益，试用资格和期限以 Overleaf 实际返回为准。详见 [Overleaf 权益说明](https://docs.overleaf.com/getting-started/free-and-premium-plans/premium-features)。

<p align="center">
  <img src="docs/images/accounts-light.png" alt="账号工作台：计划、有效期、凭据状态、无感换号与运行日志" width="1000">
</p>

<a id="architecture"></a>

## 工作原理

### 各部分如何连接

<p align="center">
  <img src="docs/images/architecture.png" alt="Overleaf Account Switcher 架构与功能联动" width="1200">
</p>

工作台把操作交给 Rust 服务，由任务编排层管理并发、账号锁和执行进度。账号与项目模块复用 Overleaf HTTP 接口、浏览器自动化和扩展桥：HTTP 读取身份、订阅与项目状态，独立浏览器完成登录和订阅操作，Chrome 扩展负责日常浏览器中的无感换号。

账号数据保存在本机，密码、Cookie、Git token 和卡片敏感字段由密钥库保存。换号成功后可同步 skills 凭据；agent 再通过 Cookie 读取和编译项目，通过 Git 提交修改、查看历史。桌面版在本机运行服务，Docker 在容器中运行服务，agent 仍在宿主机使用。

### 后端如何执行任务

以无感换号为例，一次操作会经过以下实际执行链路。绿色箭头是执行和写回流程，恢复分支在 Cookie 缺失或失效时介入。

<p align="center">
  <img src="docs/images/backend-workflow.png" alt="后端执行流程：任务准入、身份核验、Cookie 恢复、项目迁移、扩展回执、状态写回与 skills 同步" width="1200">
</p>

- **请求与任务分离。** `runtime/server` 接收请求，`api` 分派操作；`runtime/tasks` 登记账号锁、进度和重试信息。耗时操作在后台执行，SSE 持续把任务快照送回界面。
- **登录态恢复共用一条链路。** `accounts/session` 核验身份，`api/credential_jobs` 调度独立浏览器恢复 Cookie；需要 reCAPTCHA 或邮箱验证时等待用户输入，恢复后继续原任务。这套浏览器批次也用于改密和 Git token 操作。
- **先确认远端结果，再写回本地。** `projects/migration` 按需复制或重新加入项目并核验访问；扩展完成写 Cookie、刷新页面和读取有效期后，才写回当前账号。随后刷新订阅，并按需同步 skills。订阅读取或 skills 同步失败会单独提示，不把已经完成的换号伪报为失败。

注册与订阅由 `workflows/registration` 定义步骤，`api/registration` 驱动浏览器并核验页面状态；卡片和地址由 `resources` 管理，账号导入导出由 `accounts/io` 处理。它们复用 `storage` 的持久化与密钥库。账号网络操作结束后，提交锁内重新读取最新数据再保存，避免并发任务互相覆盖；取消浏览器任务时，先完成会话清理再释放账号锁。

<a id="download"></a>

## 下载

从 [GitHub Releases](https://github.com/YuanzAAi/overleaf-account-switcher/releases) 选择对应平台：

| 平台 | 下载文件 | 使用方式 |
| --- | --- | --- |
| Windows x64 | `overleaf-windows-x64.exe` | 直接运行，服务与扩展已内置。 |
| Windows x64 便携包 | `overleaf-windows-x64-portable.zip` | 解压后运行 `overleaf-desktop.exe`，包内另附独立服务与扩展。 |
| macOS Apple Silicon | `overleaf-macos-arm64-portable.zip` | 解压后打开 `.app`。 |
| macOS Intel | `overleaf-macos-x64-portable.zip` | 解压后打开 `.app`。 |
| Docker | `ghcr.io/yuanzaai/overleaf-account-switcher:latest` | 使用 [Docker Compose](#docker)。 |

macOS 首次打开若提示无法验证开发者，可在确认下载来源后，从“系统设置 → 隐私与安全性 → 仍要打开”启动。

<a id="quick-start"></a>

## 快速开始

### 环境准备

- **Windows：** Windows 10 / 11 x64、Google Chrome、[WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。
- **macOS：** Google Chrome，默认安装在 `/Applications`。
- **skills 联动：** 默认已有 Codex 或 Claude Code；Python 3.10+ 和 Git 可从 `PATH` 找到。

预编译包不需要 Rust、Node.js 或本地 TeX 环境。请使用当前稳定版 Chrome；登录和注册会自动创建独立临时浏览器档案，无需自己寻找临时 Chrome 的路径。

### 连接与使用

1. **打开程序。** Windows 运行 EXE，macOS 打开 `.app`。服务启动后，也可在浏览器访问 `http://127.0.0.1:8765/ui/`。
2. **加载扩展。** 在目标 Chrome 档案中打开 `chrome://extensions/`，启用“开发者模式”，选择“加载已解压的扩展程序”，使用应用引导中显示的目录。
3. **添加账号。** 导入账号文件、粘贴 Cookie，或用账号密码登录。遇到 reCAPTCHA 或邮箱验证时，按任务提示完成验证。
4. **连接 agent。** 在“设置运行”中安装所需 skills；根据需要勾选“换号时迁移项目”“同步 skills 账号”，执行无感换号。

<details>
<summary>查看扩展连接引导</summary>

<p align="center">
  <img src="docs/images/extension-setup.png" alt="Chrome 扩展加载与连接检测" width="820">
</p>

单文件 EXE 自动释放扩展；ZIP 用户也可加载包内的 `chrome_extension`。以应用显示的路径为准。更新扩展文件后，在 Chrome 扩展页点击“重新加载”。

</details>

关闭桌面窗口会隐藏到托盘，完整退出请使用托盘菜单“退出”。用户数据保存在程序目录之外。

### 更新

在“设置运行 → 应用更新”中检查新版本。桌面端选择“下载并更新”，程序会校验下载内容、替换程序并重启，账号数据保留；请先完成或取消仍在运行的任务。Docker 用户可在同一处复制 Compose 更新命令，在部署目录执行。若使用固定版本镜像，先将 `.env` 中的 `OVERLEAF_IMAGE` 改为页面显示的目标镜像。

<a id="skills"></a>

## Overleaf Skills

[overleaf-skills](https://github.com/YuanzAAi/overleaf-skills) 也可独立使用。Codex 与 Claude Code 共用 Python CLI，分别使用各自格式的 skills 入口，不需要额外的 MCP 服务或 pip 依赖。

<p align="center">
  <img src="docs/images/skills.png" alt="Codex 与 Claude Code 的 skills 安装和账号同步" width="1000">
</p>

安装并同步账号后，可以直接告诉 agent：

> 用 overleaf-skills 列出我的 Overleaf 项目。先读论文的 main.tex，再结合本地实验结果修改结果章节；给我确认后，提交到 Overleaf，编译并把 PDF 下载回来。

Cookie 用于读取、编译与下载；编辑和历史操作使用 Git，需要相应的 [Overleaf Git 集成权限](https://docs.overleaf.com/integrations-and-add-ons/git-integration-and-github-synchronization/git-integration)，有 git token 即可。账号同步需要 Cookie 与 Git token，缺少时会提示并保留原 skills 状态。

同步对后续命令生效。项目复制后 ID 会变化，让 agent 重新列出项目并确认目标即可。

<a id="docker"></a>

## Docker

安装 Docker Engine / Docker Desktop 和 Compose v2，下载本仓库。在根目录创建 `.docker-keyring-password`，写入一行自行生成的强密码，用于持久化密钥库；请妥善备份。

在根目录 `.env` 中设置镜像：

```dotenv
OVERLEAF_IMAGE=ghcr.io/yuanzaai/overleaf-account-switcher:latest
```

```sh
docker compose -f docker/compose.yml pull
docker compose -f docker/compose.yml up -d
```

在宿主机 Chrome 打开 **http://127.0.0.1:8765/ui/**，即可使用与桌面版相同的工作台。需要浏览器无感换号时，在这个 Chrome 档案中加载仓库里的 `chrome_extension`，扩展连接本机 `9876` 端口。

登录与注册任务使用容器内的独立浏览器。遇到 reCAPTCHA 时，点击任务等待区或“设置运行”中的**打开任务浏览器**完成验证，日常操作仍在 Web UI 中进行。

Compose 会挂载宿主机 Codex / Claude Code 的 skills 和账号状态目录，agent 继续在宿主机运行。使用自定义目录时，启动前设置 `CODEX_HOME` 或 `CLAUDE_CONFIG_DIR`；宿主机需有 Python 和 Git，Docker 需有这些目录的读写权限。

<details>
<summary>数据、端口与源码构建</summary>

账号数据保存在 `data` 命名卷，`docker compose down` 保留数据，`down -v` 会删除数据。密钥库密码应与数据卷一起保管。

`OVERLEAF_WEB_PORT` 和 `OVERLEAF_BROWSER_PORT` 可调整 Web UI 与任务浏览器的宿主机端口。浏览器扩展使用 `localhost:9876`，请保持该端口可连接。

默认端口只绑定本机。远程部署请通过 SSH 隧道等受控方式访问，不要将 API 或任务浏览器直接暴露公网。skills 挂载对应 Docker 所在机器，跨机器使用时需在 agent 所在机器配置凭据。

从源码构建：

```sh
docker compose -f docker/compose.yml up -d --build
```

</details>

<a id="faq"></a>

## 常见问题

### Chrome 和 skills 路径要手动配置吗？

标准安装通常不需要。Windows 自动查找当前用户和 Program Files 下的 Chrome，macOS 查找 `/Applications/Google Chrome.app`。Chrome 136 起对默认档案的远程调试限制不影响这里使用的独立临时档案。

<details>
<summary>默认目录与自定义路径</summary>

| 项目 | 默认位置 / 配置 |
| --- | --- |
| Windows 数据 | `%USERPROFILE%/.OverleafAccountSwitcher/data`，临时档案在同级 `tmp` |
| macOS 数据 | `~/.local/share/.OverleafAccountSwitcher/data`，遵循 `XDG_DATA_HOME` |
| 自定义数据 / 临时目录 | `OVERLEAF_SWITCHER_DATA_DIR` / `OVERLEAF_SWITCHER_TMP_DIR` |
| Chrome 可执行文件 | `OVERLEAF_SWITCHER_CHROME_PATH`；macOS 指向 `.app/Contents/MacOS/Google Chrome` |
| Chrome 档案根目录 | `OVERLEAF_SWITCHER_CHROME_USER_DATA_DIR`，指包含 `Local State` 的目录 |
| Codex skills | `~/.codex/skills/overleaf-skills`，遵循 `CODEX_HOME` |
| Claude Code skills | `~/.claude/skills/overleaf-skills`，遵循 `CLAUDE_CONFIG_DIR` |
| skills 状态 / 缓存 | `<agent-home>/overleaf-skills/state.json` / `cache` |

环境变量需在启动程序前设置。skills 安装器在 Windows 调用 `python`，macOS 调用 `python3`；从 Finder 启动时，应确保应用可找到相应命令。

自定义 `OVERLEAF_SKILL_STATE_DIR` 时，应用与 agent 需使用同一目录。显式凭据和 `OVERLEAF_SESSION` / `OVERLEAF_GIT_TOKEN` 环境变量优先于状态文件；换号后仍使用旧账号时，先检查这些设置。

</details>

### 项目迁移会保留哪些内容？

自有项目复制到目标账号；共享项目按权限尝试重新加入，必要时采用复制策略。副本的 ID 会变化，历史、评论和协作关系不一定原样保留，部分迁移路径会启用链接共享或发送邀请。重要项目请先备份，迁移后检查目标内容和权限。

### 账号和凭据保存在哪里？

账号密码、Cookie、Git token、卡号与安全码存入系统密钥库，内部 JSON 保存引用。显式导出文件和 skills 的 `state.json` 含有明文凭据，请勿公开或提交到 Git。跨机器迁移使用导出 / 导入，不要只复制内部 JSON。

本项目与 Overleaf 官方无隶属关系。请只操作自己拥有或获得授权的账号和项目，遵守试用资格与支付规则；订阅操作可能产生真实费用。

<a id="structure"></a>

## 项目结构

| 目录 | 职责 |
| --- | --- |
| `apps/web` | 账号、注册、资源与设置页面，共用的交互状态、任务日志和主题。 |
| `apps/desktop` | Tauri 桌面窗口、托盘、文件对话框、剪贴板与服务进程管理。 |
| `crates/core` | 账号、项目、卡片等领域模型与基础规则。 |
| `crates/storage` | 数据目录、账号与资源持久化、系统密钥库。 |
| `crates/browser` | Chrome 档案、CDP 自动化、浏览器生命周期与扩展桥。 |
| `crates/overleaf-api` | Overleaf 身份、订阅、项目接口与响应解析。 |
| `crates/workflows` | 注册、订阅和项目迁移的流程定义。 |
| `crates/service` | 本地 API、账号操作、任务编排、并发执行与 skills 联动。 |
| `chrome_extension` | 浏览器会话操作与 WebSocket 桥接。 |
| `docker` / `scripts` | 容器环境与跨平台打包脚本。 |
| `docs` | 产品截图与工作原理图。 |

<a id="development"></a>

## 开发与构建

Rust 工作区承载业务与服务，Tauri 2 提供桌面窗口、托盘和原生交互，原生 JavaScript / CSS 界面由 esbuild 打包。

需要当前 stable Rust（最低 1.88）、Node.js 22 和 npm。Windows 需 MSVC 与 Visual Studio C++ Build Tools；macOS 需 Xcode Command Line Tools。

```sh
npm --prefix apps/web ci
npm --prefix apps/desktop ci
```

Windows：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package-desktop.ps1
```

macOS：

```sh
bash scripts/package-macos.sh aarch64-apple-darwin
# Intel: bash scripts/package-macos.sh x86_64-apple-darwin
```

产物位于 `target/desktop-package/`。[构建工作流](.github/workflows/build.yml) 支持手动构建；推送 `v*` 标签会构建并发布桌面产物与 Docker 镜像。

## 致谢

- [Overleaf](https://www.overleaf.com/) 与 TeX / LaTeX 生态。
- [overleaf-mcp-plus](https://pypi.org/project/overleaf-mcp-plus/) 和 [mjyoo2/OverleafMCP](https://github.com/mjyoo2/OverleafMCP)，为 Overleaf 工具集成提供参考。

## License

[MIT](LICENSE)

## 认可社区

<a href="https://linux.do"><img src="https://cdn3.ldstatic.com/original/4X/d/1/4/d146c68151340881c884d95e0da4acdf369258c6.png" alt="LINUX DO" height="32"></a>
