# sys perms：从 compose 派生宿主 bind 挂载权限

> 对应 issue [#29](https://github.com/galaxio-labs/galaxy-ops/issues/29)。
> 纯增量：不改现有命令的行为；`localize` / `run start` 的只读告警与 preflight 都可用
> `--no-perms-check` 关闭。

## 问题

docker-compose 系统常见这个组合：

- 业务容器以**固定非 root 身份**运行（`user: "999:999"`）；
- 配置 / 运行态 / 投放目录用 **bind 挂载**（`./configs:/config`、`./packages:/packages:ro`）；
- Linux 上 bind 挂载**不改变属主**，而 `localize` / `prj sync` / `prj restore` / `prj upgrade`
  都以**调用账号**写系统目录。

于是「谁先创建就归谁」：`docker compose up` 会以 root 建缺失的挂载源；root 跑过一次
`localize` / `restore`；tar 解包保住的是解包账号的属主。结果不是容器读不到私钥，就是
日志/库对属组不可写、安装包无法覆盖同名旧文件。

每个栈自己写 `align-host-perms.sh` 的问题是：**名单会与 compose 实际挂载漂移**。
而 `docker compose config --format json` 已经给出了完整契约，且不需要拉镜像。

## 派生契约

`gops sys perms` 调用（与 `gops run` 共用同一套 compose 解析与项目目录锚定）：

```bash
docker compose [-f sys/compose.yaml] --project-directory <系统根> config --format json
```

从中读取每个服务的：

| compose 字段 | 用途 |
|---|---|
| `services.<name>.user` | 容器身份（`uid:gid`）；缺失时 `docker image inspect <image> --format {{.Config.User}}` |
| `services.<name>.volumes[].type == "bind"` | 只处理 bind；named volume 不碰 |
| `services.<name>.volumes[].source` | 宿主真实路径（`.env` 已代入、相对路径已按 project-directory 展开） |
| `services.<name>.volumes[].read_only` | 读写 / 只读语义 |

身份解析顺序（先命中先用）：

1. `sys-prj.yml: perms.container_uid/gid` —— 全局宿主映射（rootless / userns-remap）；
2. `sys-prj.yml: perms.services.<name>.uid/gid` —— 单服务“有效身份”，**只影响对齐，不改运行时**；
3. compose `services.<name>.user`；
4. 镜像 `Config.User`（`docker image inspect`）。

规则：

- `uid:gid` 必须是数字；`uid` 省略 group 时 gid 取 `0`（与 docker 一致）；
- **`Config.User` 为空 = root（0:0）**，不是“读取失败”。root 不受宿主文件模式限制，
  该服务会打印 `[INFO]` 并**跳过对齐**：强行对齐到 `0640` 反而会弄坏由非 root 进程读取的挂载
  （`htpasswd` 由 nginx worker 读、`/docker-entrypoint-initdb.d` 由降权后的 postgres 读）；
- `docker image inspect` 失败（通常是镜像未拉取）才告警，并提示用 `perms.services.<name>` 声明；
- 用户名 / 组名、镜像 `Config.User` 非数字 → 跳过该服务并告警，提示改用逐服务声明；
- `perms.services` 里的服务名在 compose 中不存在 → 告警（拼写错误不会静默）；
- 全局 `perms.container_uid/gid` 存在时，`perms.services` 被忽略（宿主映射对所有服务一致）；
- rootless / userns-remap：容器 uid 不等于宿主 uid，**报错**并提示配置
  `sys-prj.yml: perms.container_uid/gid`（不静默按 999 对齐）。

## 对齐规则

| 需求 | 文件 | 目录 |
|---|---|---|
| 读写（`read_only != true`） | `0660` | `2770`（setgid） |
| 只读（`read_only == true`） | `0640` | `2750` |

- 属主 = 部署账号（`SUDO_UID` / euid），属组 = 容器 gid；
- **`sys-prj.yml: secrets.file`**：属主**始终是部署账号**（不 chown 给容器 uid —— 那需要提权）。
  - 不与容器共享（或只给 root 容器用）：`secrets.mode: "0600"`（默认）；
  - 与非 root 容器共享：`secrets.mode: "0640"` + **属组 = 容器 gid**，部署账号按 owner 读、
    容器按 group 读；`--apply` 只会 chmod/chgrp，不会改属主；
  - 一次性前置（管理员，不是 gops）：部署账号加入容器 gid；`.wist` 用 setgid 继承该组；
  - `perms` **不会创建**缺失的密钥文件；`0600` + 非 root 容器判为**配置错误**
    （容器读不到），必须改 `secrets.mode`，不能靠改权限修复；
  - 多个非 root 消费者 uid 不同时，单文件无法同时满足，按配置冲突报错。
- **以 root 运行的服务不参与对齐**（root 可读写任何宿主文件；对齐只会收紧模式）；
- **容器自建的**（属主 = 容器 uid 且不等于部署账号）不动 —— 保证零 sudo 幂等；
- 递归遍历挂载源（不按文件名列举），运行态新长出的文件自动纳入；
- 只改**挂载根及其子树**，不改挂载根的父目录；
- 不跟随符号链接，也不修改链接本身；
- `--apply` 后**再探一次**；仍有残留（ACL / 只读挂载 / 权限不足）→ 失败，不静默通过。

「已对齐」的判定：属主是部署账号、属组是容器 gid、模式**等于**上表；
或属主是容器身份（容器自建）。检查同时验证容器身份按 owner/group/other 位**实际**可读/可写。

## 命令与生命周期

```bash
gops sys perms            # 默认 --check：只 stat，未对齐 exit 1
gops sys perms --apply    # 幂等修复
gops sys perms --json     # 机器可读（CI / 上层编排）
```

| 位置 | 行为 |
|---|---|
| `gops sys localize`（compose） | 末尾 `--check`，**只告警、不 chown**，不改退出码 |
| `gops run start`（compose） | preflight：未对齐即停，打印 `gops sys perms --apply` |
| `gops prj restore` | 完成后对还原到的系统 `--apply`（尽力而为，失败只告警） |
| `gops prj upgrade` | `up` 前 `--apply`（否则容器只会无限重启）；成功后再对齐一次 |
| `gops prj diagnose` | 增加 `perms.aligned` / `perms.drift` 一节；默认 WARN，`--strict` 为 FAIL |

关闭开关：`sys localize --no-perms-check`、`run start --no-perms-check`。

## 覆盖项（`sys-prj.yml`）

```yaml
perms:
  # rootless / userns-remap：容器内身份映射到宿主后的 uid/gid（必须同时给）
  container_uid: 100999
  container_gid: 100999
  # 按服务声明“有效容器身份”（仅用于对齐，不改运行时）。
  # 典型：镜像 Config.User 为空（默认 root），但入口脚本会降权：
  #   官方 postgres 以 root 启动，gosu 到 999 之后才读 /docker-entrypoint-initdb.d
  services:
    postgres:
      uid: 999
      gid: 999
  # 允许处理系统根之外的 bind source（绝对路径或相对系统根）
  allow_external:
    - /srv/shared/packages
```

缺省不写 = 完全按 compose 派生（root 服务自动跳过对齐）。
`perms.services.<name>` 的键是 **compose 服务名**（不是 `container_name`）；它给的是
**容器内身份**，rootless / userns-remap 仍需 `container_uid/gid` 的全局宿主映射。

> 密钥共享策略在 `secrets:` 节：`secrets.mode: "0640"`（默认 `0600`）；
> 属主始终是部署账号，容器按属组读。该字段需要 gops ≥ 2.9.0。

## 安全边界

- 只处理**系统根内**的 bind `source`；根外必须显式 `allow_external`；
- **bind source 等于或包含系统根时直接跳过并告警**（典型 `.:/app`）：递归对齐会把
  `docs/`、`sys/`、脚本、`.git` 等交付内容一起 chmod；请把挂载细化到配置/运行态子目录；
- 同一路径被**不同容器身份**挂载时属于配置冲突：保留第一个身份继续检查，但整次
  `--check` / `--apply` 判**失败**（单一属组无法同时满足两个容器），不静默放过；
- 不碰 named volume、不尝试进入容器层；
- 平台只支持 Linux（macOS / OrbStack 打印 `[SKIP]` 并 exit 0）；
- `--apply` 是显式动作；自动执行只发生在 `restore` 完成后与 `upgrade` 的 `up` 前。

## 已知不能覆盖

SELinux 标签（`:z` / `:Z`）、NFS root squash、named volume、容器层内写入 —— 这些只提示，不代做。
