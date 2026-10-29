#!/usr/bin/env bash
# 依赖隔离检查：boa_* 只允许出现在执行引擎实现模块内。
#
# 动机：阶段 B 将用自研虚拟机替换阶段 A 的引擎实现。
# 一旦 boa 的类型泄漏到其他模块，替换成本会从「换一个实现」变成「全局重构」。
# 因此在每次提交前机械校验，而不是依靠人工自觉。
#
# 用法：scripts/check-isolation.sh
# 退出码：0 表示通过，1 表示存在越界引用。

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
src_dir="${repo_root}/src"
engine_dir="${src_dir}/engine"

# 收集除引擎实现模块之外的所有 Rust 源码中的 boa 引用
violations="$(
  grep -rn --include='*.rs' -E '\bboa_(engine|parser|gc)\b' "${src_dir}" \
    | grep -v "^${engine_dir}/" \
    || true
)"

if [ -n "${violations}" ]; then
  echo "依赖隔离检查失败：boa_* 只能出现在 src/engine/ 下" >&2
  echo "${violations}" >&2
  exit 1
fi

echo "依赖隔离检查通过"
