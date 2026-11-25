---
name: real-data-trial
description: 需要在用户的真实资料库上验证改动效果时使用。保证只在副本上运行，不碰正在使用的资料库和运行目录，结果只报计数和差异。
---

# 在真实数据的副本上试验

真实资料库在 `~/Library/Application Support/Candid/timeline/`。**永远不要**对它或对用户的运行目录执行写入、迁移或删除。

## 做法一：对照实验（改了整理层、提示词、模型配置时）

`candid-lab` 直接驱动生产代码，从样本包克隆干净副本运行，真实库只被读。方法说明见 `docs/plans/data-layer-lab.md` 第 1 节。

```bash
cargo run --manifest-path core/Cargo.toml -p candid-lab -- pack "$HOME/Library/Application Support/Candid/timeline" <样本名>
```

```bash
cargo run --manifest-path core/Cargo.toml -p candid-lab -- run <样本包> <实验.json> <运行名>
```

```bash
cargo run --manifest-path core/Cargo.toml -p candid-lab -- diff <运行A> <运行B>
```

工具目录默认 `~/CandidLab`（`CANDID_LAB` 可改）。样本包、缓存、运行结果都含真实数据，只留在本机，不进仓库。

## 做法二：独立核心（要验证升级、查询、接口时）

1. 把资料库复制到临时目录（APFS 上 `cp -c` 是写时复制，不占空间）。
2. 用自己的运行目录起一个独立核心，`--root` 指向副本：

```bash
CANDID_RUN_DIR=<临时运行目录> candid serve --root <副本目录>
```

3. 之后所有 `candid` 命令都带同一个 `CANDID_RUN_DIR`，确保连的是这个核心而不是用户正在用的那个。

## 动手前自查

- `--root` 指向的是副本吗？路径里不应出现 `Application Support/Candid/timeline`。
- 设了自己的 `CANDID_RUN_DIR` 吗？
- 用完后停掉自己起的核心，删掉临时副本。

## 报告

只给计数和差异（多少条、增减多少、哪类变了），不贴任何消息、截图文字或人名。
