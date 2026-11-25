---
name: check-changes
description: 改完代码或文档后，跑这次改动必须跑的检查并判断结果。用于「检查一下」「跑测试」「能不能提交」，以及任何改动收尾时。
---

# 检查改动

1. 先看会跑什么：

```bash
scripts/check.sh --list
```

2. 再执行。Rust 和 Swift 的套件可能要几分钟，放后台跑：

```bash
scripts/check.sh
```

3. 按退出码处理：

| 退出码 | 含义 | 做什么 |
|---|---|---|
| 0 | 全过 | 可以汇报完成 |
| 1 | 有检查失败 | 读失败输出，改代码。不要放松断言、加跳过或删测试 |
| 2 | 有检查因缺环境没跑成 | 按提示补环境；补不了就在汇报里明确写「某项没跑成」，不能说通过 |

## 快照不一致时

全流程测试或契约测试报快照不一致，先判断这次改动是否**本来就要**改接口、输出字段或提示词。

- 不是有意的：这是 bug，改代码。
- 是有意的：重新生成，并在提交说明里写清变了什么、谁受影响；同步更新 `docs/contracts/README.md` 或对应规格。

```bash
UPDATE_SNAPSHOTS=1 cargo test --manifest-path core/Cargo.toml -p candid-service --test lifecycle --test mcp_contract
```

```bash
UPDATE_SNAPSHOTS=1 cargo test --manifest-path core/Cargo.toml -p candid-cli --test contract
```

重新生成后用 `git diff core/fixtures` 逐个看差异，确认只有预期的变化。

## 汇报

用主人能判断的话写：用户能看到什么变化、跑了哪些检查、结果如何、哪项没跑成、有什么需要主人决定。不贴代码。
完整规则见 `docs/engineering.md` 第 3 节。
