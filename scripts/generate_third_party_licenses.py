#!/usr/bin/env python3
"""THIRD_PARTY_LICENSES.md の Cargo 依存クレート一覧を再生成する。

`cargo metadata` から依存クレートの名前・バージョン・ライセンス (SPDX) を取得し、
マーカーで囲まれた一覧部分だけを書き換える。ネイティブライブラリの節は手書きの
まま維持する。
"""

import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD_PARTY_LICENSES.md"
BEGIN = "<!-- BEGIN CARGO DEPENDENCIES (auto-generated) -->"
END = "<!-- END CARGO DEPENDENCIES (auto-generated) -->"


def cargo_table() -> str:
    """Cargo.lock に記録された依存クレートのライセンス一覧を Markdown 表で返す。"""
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=ROOT,
        )
    )
    packages = sorted(metadata["packages"], key=lambda p: (p["name"], p["version"]))
    lines = ["| クレート | バージョン | ライセンス (SPDX) |", "|---|---|---|"]
    for package in packages:
        license_id = package.get("license") or "UNKNOWN"
        lines.append(f"| {package['name']} | {package['version']} | {license_id} |")
    return "\n".join(lines)


def main() -> None:
    text = OUT.read_text()
    begin = text.index(BEGIN) + len(BEGIN)
    end = text.index(END)
    OUT.write_text(text[:begin] + "\n\n" + cargo_table() + "\n\n" + text[end:])
    print(f"updated {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
