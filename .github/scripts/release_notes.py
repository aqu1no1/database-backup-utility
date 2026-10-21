#!/usr/bin/env python3
"""Valida versionamento unificado e extrai release notes do CHANGELOG.md."""
from __future__ import annotations

import argparse
import re
import sys
import tomllib
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def extract(version: str, root: Path = ROOT) -> str:
    match = re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version)
    if not match:
        raise ValueError("Versão inválida. Use vMAJOR.MINOR.PATCH, por exemplo v0.1.0.")
    plain = version[1:]
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    found_version = cargo["workspace"]["package"]["version"]
    if found_version != plain:
        raise ValueError(f"Cargo.toml informa {found_version}; esperado {plain}.")
    for member in cargo["workspace"].get("members", []):
        file = root / member / "Cargo.toml"
        pkg = tomllib.loads(file.read_text(encoding="utf-8"))["package"]
        declared = pkg.get("version")
        if declared != {"workspace": True} and declared != plain:
            raise ValueError(f"{member} não usa a versão unificada {plain}.")
    changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    header = re.compile(r"^## \[(?P<version>[^\]]+)\](?:\s+-\s+(?P<date>\d{4}-\d{2}-\d{2}))?\s*$", re.M)
    sections = list(header.finditer(changelog))
    matching = [m for m in sections if m.group('version') == plain]
    if len(matching) != 1:
        raise ValueError(f"CHANGELOG.md precisa conter uma seção única ## [{plain}] - AAAA-MM-DD.")
    section = matching[0]
    if not section.group('date'):
        raise ValueError('A seção da versão deve ter uma data real no formato AAAA-MM-DD.')
    try:
        date.fromisoformat(section.group('date'))
    except ValueError as exc:
        raise ValueError('Data inválida na seção da versão.') from exc
    i = sections.index(section)
    end = sections[i + 1].start() if i + 1 < len(sections) else len(changelog)
    body = changelog[section.end():end].strip()
    lines = [line.strip() for line in body.splitlines()]
    if not any(line.startswith('- ') for line in lines):
        raise ValueError('A seção da versão não contém itens de mudança publicados.')
    if not re.search(r'^### (Added|Changed|Deprecated|Removed|Fixed|Security)\b', body, re.M):
        raise ValueError('Use pelo menos uma categoria Keep a Changelog na versão.')
    return f"## {version}\n\n{body}\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', required=True, help='Ex.: v0.1.0')
    parser.add_argument('--output', help='Arquivo .md de saída; se ausente, escreve no stdout')
    args = parser.parse_args()
    try:
        result = extract(args.version)
    except (ValueError, KeyError, FileNotFoundError) as error:
        print(f'ERRO: {error}', file=sys.stderr)
        return 1
    if args.output:
        Path(args.output).write_text(result, encoding='utf-8')
    else:
        print(result)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
