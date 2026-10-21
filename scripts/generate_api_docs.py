#!/usr/bin/env python3
"""Generate the contract API reference from the Soroban contract sources.

Reads every public function inside a `#[contractimpl]` block and every
`#[contracttype]` struct/enum reachable from those signatures, then writes:

  docs/api/openapi.json      OpenAPI 3.0.3 specification
  docs/api/curl-examples.md  One curl command per operation
  docs/api/index.html        Swagger UI explorer (filter + "Try it out")

The HTTP gateway that exposes these operations is deployed outside this
repository (see docs/api-performance.md). The spec therefore describes each
contract function as `POST /{contract}/{function}` with a JSON body whose
keys are the Rust argument names. Gateway routes must match this shape.

Standard library only. Usage:

  python3 scripts/generate_api_docs.py          # regenerate the files
  python3 scripts/generate_api_docs.py --check  # exit 1 if they are stale (CI)
"""

import argparse
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CONTRACTS_DIR = REPO_ROOT / "contracts"
OUT_DIR = REPO_ROOT / "docs" / "api"

DEFAULT_SERVER = "http://localhost:8080/v1"
INT_TYPES = {"u32", "i32", "u64", "i64", "u128", "i128"}
ENV_TYPE = "Env"

# Display names for the contract directories under contracts/.
CONTRACT_TITLES = {
    "asset-registry": "AssetRegistry",
    "engineer-registry": "EngineerRegistry",
    "lending": "Lending",
    "lifecycle": "Lifecycle",
}


def read_production_source(path):
    """Return the file text up to the first column-0 `#[cfg(test)]` block."""
    kept = []
    for line in path.read_text().splitlines():
        if line.startswith("#[cfg(test)]"):
            break
        kept.append(line)
    return "\n".join(kept)


def contractimpl_blocks(text):
    """Yield the text of each `#[contractimpl]` impl block (up to its column-0 `}`)."""
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        if lines[i].strip() == "#[contractimpl]":
            end = i + 1
            while end < len(lines) and lines[end] != "}":
                end += 1
            yield "\n".join(lines[i + 1 : end + 1])
            i = end + 1
        else:
            i += 1


def split_top_level(text):
    """Split on commas that are not nested inside <>, () or []."""
    parts, depth, current = [], 0, ""
    for ch in text:
        if ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(current.strip())
            current = ""
        else:
            current += ch
    if current.strip():
        parts.append(current.strip())
    return [p for p in parts if p]


def parse_functions(block):
    """Return (name, [(arg_name, rust_type)], return_type_or_None) for each pub fn."""
    functions = []
    for match in re.finditer(r"^\s*pub fn (\w+)\s*\(", block, re.M):
        name = match.group(1)
        if name.startswith("__"):
            continue  # Deploy-time constructors are not invokable operations.
        pos, depth = match.end(), 1
        while depth:
            if block[pos] == "(":
                depth += 1
            elif block[pos] == ")":
                depth -= 1
            pos += 1
        arg_text = block[match.end() : pos - 1]
        header = block[pos : block.index("{", pos)].strip()
        ret_match = re.match(r"->\s*(.+)$", header, re.S)
        ret = ret_match.group(1).strip() if ret_match else None

        args = []
        for part in split_top_level(arg_text):
            arg_name, _, arg_type = part.partition(":")
            arg_name, arg_type = arg_name.strip(), arg_type.strip()
            if arg_type == ENV_TYPE:
                continue
            args.append((arg_name, arg_type))
        functions.append((name, args, ret))
    return functions


def parse_contracttypes(text):
    """Return {name: definition} for every #[contracttype] struct or enum in text."""
    types = {}
    lines = text.splitlines()
    pending = False
    i = 0
    while i < len(lines):
        stripped = lines[i].strip()
        if stripped == "#[contracttype]":
            pending = True
        elif pending and re.match(r"pub (struct|enum) \w+", stripped):
            kind, name = re.match(r"pub (struct|enum) (\w+)", stripped).groups()
            body = []
            i += 1
            while i < len(lines) and lines[i] != "}":
                body.append(lines[i].split("//")[0].rstrip())
                i += 1
            types[name] = {"kind": kind, "body": body}
            pending = False
        elif stripped and not stripped.startswith(("#[", "///", "//")):
            pending = False
        i += 1
    return types


class SchemaBuilder:
    """Turns Rust types into OpenAPI schemas and tracks which named types are used."""

    def __init__(self, contract_types):
        # contract_types: {contract_dir: {type_name: definition}}
        self.contract_types = contract_types
        self.components = {}
        self.queue = []
        self.unresolved = set()

    def component_name(self, contract, name):
        return f"{CONTRACT_TITLES.get(contract, contract)}.{name}"

    def resolve(self, contract, name):
        """Find a named type, preferring the contract that references it."""
        if name in self.contract_types.get(contract, {}):
            return contract
        for other, types in self.contract_types.items():
            if name in types:
                return other
        return None

    def ref(self, contract, name):
        owner = self.resolve(contract, name)
        if owner is None:
            if (contract, name) not in self.unresolved:
                print(f"generate_api_docs: warning: type `{name}` in `{contract}` is not defined in source", file=sys.stderr)
            self.unresolved.add((contract, name))
            return {"description": f"Type `{name}` is referenced but not defined in the contract source."}
        comp = self.component_name(owner, name)
        if comp not in self.components and (owner, name) not in self.queue:
            self.queue.append((owner, name))
        return {"$ref": f"#/components/schemas/{comp}"}

    def schema(self, contract, rust_type):
        # Drop module paths such as `soroban_sdk::` so qualified names map like bare ones.
        ty = re.sub(r"\b(?:[a-z_][a-z0-9_]*::)+", "", rust_type.strip())
        if ty.startswith("Option<") and ty.endswith(">"):
            inner = self.schema(contract, ty[len("Option<") : -1])
            return {**inner, "nullable": True}
        if ty.startswith("Vec<") and ty.endswith(">"):
            return {"type": "array", "items": self.schema(contract, ty[len("Vec<") : -1])}
        if ty.startswith("Map<") and ty.endswith(">"):
            key, value = split_top_level(ty[len("Map<") : -1])
            return {
                "type": "object",
                "description": f"Map from `{key}` to `{value}`, encoded as a JSON object.",
            }
        if ty.startswith("(") and ty.endswith(")"):
            items = split_top_level(ty[1:-1])
            return {
                "type": "array",
                "description": "Tuple (" + ", ".join(items) + "), encoded as a JSON array.",
                "items": {},
            }
        if ty in INT_TYPES:
            return {"type": "integer", "format": ty}
        if ty == "bool":
            return {"type": "boolean"}
        if ty == "Address":
            return {
                "type": "string",
                "description": "Stellar account (G...) or contract (C...) address in strkey form.",
            }
        if ty == "Symbol":
            return {"type": "string", "maxLength": 32, "description": "Soroban Symbol (max 32 characters)."}
        if ty == "String":
            return {"type": "string", "description": "UTF-8 string."}
        if ty == "Bytes":
            return {"type": "string", "description": "Hex-encoded byte string."}
        m = re.fullmatch(r"BytesN<(\d+)>", ty)
        if m:
            size = int(m.group(1))
            return {
                "type": "string",
                "pattern": f"^[0-9a-f]{{{2 * size}}}$",
                "description": f"Hex-encoded {size} bytes ({2 * size} lowercase hex characters).",
            }
        return self.ref(contract, ty)

    def build_component(self, contract, name):
        definition = self.contract_types[contract][name]
        if definition["kind"] == "struct":
            properties, required = {}, []
            for line in definition["body"]:
                m = re.match(r"\s*pub (\w+)\s*:\s*(.+?),?\s*$", line)
                if m:
                    field, field_type = m.groups()
                    properties[field] = self.schema(contract, field_type)
                    if not field_type.startswith("Option<"):
                        required.append(field)
            schema = {"type": "object", "properties": properties}
            if required:
                schema["required"] = required
            return schema

        variants = []
        for line in definition["body"]:
            m = re.fullmatch(r"\s*(\w+)\s*(?:=\s*\d+)?\s*,?\s*", line)
            if m:
                variants.append(m.group(1))
        if not variants:
            return {"type": "object", "description": f"Enum `{name}` with data-carrying variants."}
        return {"type": "string", "enum": variants, "description": f"One of the `{name}` variants."}

    def build_all(self):
        """Resolve every queued type, including types referenced by other types."""
        while self.queue:
            owner, name = self.queue.pop(0)
            comp = self.component_name(owner, name)
            if comp in self.components:
                continue
            self.components[comp] = {}  # Placeholder prevents cycles from re-queuing.
            self.components[comp] = self.build_component(owner, name)
        return dict(sorted(self.components.items()))


def humanize(name):
    return name.replace("_", " ").capitalize()


def build_spec(version):
    contract_dirs = sorted(p for p in CONTRACTS_DIR.iterdir() if (p / "src").is_dir())
    functions_by_contract = {}
    contract_types = {}
    for contract_dir in contract_dirs:
        slug = contract_dir.name
        sources = [read_production_source(p) for p in sorted((contract_dir / "src").glob("*.rs"))]
        text = "\n".join(sources)
        if "#[contractimpl]" not in text:
            continue  # Helper crates such as contracts/shared have no contract entry point.
        contract_types[slug] = parse_contracttypes(text)
        functions, seen = [], set()
        for source in sources:
            for block in contractimpl_blocks(source):
                for function in parse_functions(block):
                    if function[0] in seen:
                        # Rust rejects duplicate method names; keep the first so output stays stable.
                        print(f"generate_api_docs: duplicate function `{function[0]}` in `{slug}`, keeping first", file=sys.stderr)
                        continue
                    seen.add(function[0])
                    functions.append(function)
        functions_by_contract[slug] = functions

    builder = SchemaBuilder(contract_types)
    paths = {}
    for slug, functions in functions_by_contract.items():
        title = CONTRACT_TITLES.get(slug, slug)
        for name, args, ret in functions:
            properties = {arg: builder.schema(slug, arg_type) for arg, arg_type in args}
            address_args = [arg for arg, arg_type in args if arg_type == "Address"]
            description = f"Invokes `{name}` on the {title} contract."
            if address_args:
                description += " Address arguments: " + ", ".join(f"`{a}`" for a in address_args) + "."
            operation = {
                "operationId": f"{slug.replace('-', '_')}_{name}",
                "summary": humanize(name),
                "description": description,
                "tags": [title],
                "responses": {
                    "400": {"$ref": "#/components/responses/ContractError"},
                    "401": {"$ref": "#/components/responses/Unauthorized"},
                },
            }
            if args:
                operation["requestBody"] = {
                    "required": True,
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": properties,
                                "required": [arg for arg, _ in args if not arg_is_option(args, arg)],
                            }
                        }
                    },
                }
            success = {"description": "Invocation succeeded."}
            if ret:
                success["content"] = {"application/json": {"schema": builder.schema(slug, ret)}}
            else:
                success["description"] = "Invocation succeeded; the function returns no value."
            operation["responses"]["200"] = success
            paths[f"/{slug}/{name}"] = {"post": operation}

    components_schemas = builder.build_all()
    spec = {
        "openapi": "3.0.3",
        "info": {
            "title": "Mainstay Contract API",
            "version": version,
            "description": (
                "Generated from the Soroban contract sources by `scripts/generate_api_docs.py`. "
                "Each operation is one public `#[contractimpl]` function. Request keys are the "
                "Rust argument names. Values follow the encodings in docs/data-model.md. "
                "Errors map to ContractError codes in docs/error-reference.md. "
                "The gateway that serves these routes is deployed separately from this repository."
            ),
        },
        "servers": [
            {
                "url": DEFAULT_SERVER,
                "description": "Gateway URL placeholder. Set MAINSTAY_API_URL or edit this entry for your environment.",
            }
        ],
        "tags": [{"name": CONTRACT_TITLES.get(s, s)} for s in functions_by_contract],
        "security": [{"apiKey": []}],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "apiKey": {
                    "type": "apiKey",
                    "in": "header",
                    "name": "X-API-Key",
                    "description": "API key issued by the gateway operator.",
                }
            },
            "responses": {
                "ContractError": {
                    "description": (
                        "The contract rejected the call. The message names a ContractError variant; "
                        "see docs/error-reference.md."
                    )
                },
                "Unauthorized": {"description": "Missing or invalid API key."},
            },
            "schemas": components_schemas,
        },
    }
    return spec, functions_by_contract, components_schemas


def arg_is_option(args, arg):
    return dict(args)[arg].startswith("Option<")


def sample_value(schema, components, depth=0):
    """Build a placeholder JSON value for a schema, used in curl examples."""
    if depth > 4:
        return None
    if "$ref" in schema:
        target = components.get(schema["$ref"].rsplit("/", 1)[-1], {})
        return sample_value(target, components, depth + 1)
    if "enum" in schema:
        return schema["enum"][0]
    kind = schema.get("type")
    if kind == "integer":
        return 1
    if kind == "boolean":
        return True
    if kind == "array":
        item = schema.get("items", {})
        return [] if not item else [sample_value(item, components, depth + 1)]
    if kind == "object":
        return {
            name: sample_value(prop, components, depth + 1)
            for name, prop in schema.get("properties", {}).items()
        }
    if kind == "string":
        pattern = schema.get("pattern")
        if pattern:
            size = int(re.search(r"\{(\d+)\}", pattern).group(1))
            return "00" * (size // 2)
        if "Stellar account" in schema.get("description", ""):
            return "G..."
        if "Hex-encoded byte" in schema.get("description", ""):
            return "00"
        return "example"
    return None


def build_curl_markdown(spec, functions_by_contract, components):
    lines = [
        "# Contract API curl examples",
        "",
        "<!-- Generated by scripts/generate_api_docs.py. Do not edit by hand. -->",
        "",
        "Each command invokes one contract function through the gateway. Set the",
        "environment variables first:",
        "",
        "```bash",
        f'export MAINSTAY_API_URL="{DEFAULT_SERVER}"',
        'export MAINSTAY_API_KEY="<your-api-key>"',
        "```",
        "",
        "Placeholder values such as `G...` and `example` must be replaced with real data.",
        "",
    ]
    for slug, functions in functions_by_contract.items():
        title = CONTRACT_TITLES.get(slug, slug)
        lines += [f"## {title}", ""]
        for name, args, _ret in functions:
            path = f"/{slug}/{name}"
            body = {arg: sample_value(spec["paths"][path]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"][arg], components) for arg, _ in args} if args else None
            lines += [f"### `{name}`", ""]
            command = [
                'curl -X POST "${MAINSTAY_API_URL}' + path + '"',
                '  -H "Content-Type: application/json"',
                '  -H "X-API-Key: ${MAINSTAY_API_KEY}"',
            ]
            if body is not None:
                command.append("  -d '" + json.dumps(body, indent=2).replace("\n", "\n  ") + "'")
            lines += ["```bash", " \\\n".join(command), "```", ""]
    return "\n".join(lines).rstrip() + "\n"


SWAGGER_HTML = """<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Mainstay Contract API Explorer</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.17.14/swagger-ui.css">
</head>
<body>
  <!-- Generated by scripts/generate_api_docs.py. Do not edit by hand. -->
  <div id="swagger-ui"></div>
  <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.17.14/swagger-ui-bundle.js"></script>
  <script>
    window.ui = SwaggerUIBundle({
      url: "openapi.json",
      dom_id: "#swagger-ui",
      filter: true,
      tryItOutEnabled: true,
      persistAuthorization: true,
      deepLinking: true,
      docExpansion: "list",
    });
  </script>
</body>
</html>
"""


def workspace_version():
    cargo = (REPO_ROOT / "Cargo.toml").read_text()
    m = re.search(r'^\s*version\s*=\s*"([^"]+)"', cargo, re.M)
    return m.group(1) if m else "0.1.0"


def render_outputs():
    spec, functions_by_contract, components = build_spec(workspace_version())
    return {
        "openapi.json": json.dumps(spec, indent=2, sort_keys=True) + "\n",
        "curl-examples.md": build_curl_markdown(spec, functions_by_contract, components),
        "index.html": SWAGGER_HTML,
    }


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="fail if committed outputs are out of date")
    args = parser.parse_args(argv)

    stale = []
    for name, content in render_outputs().items():
        target = OUT_DIR / name
        if args.check:
            if not target.exists() or target.read_text() != content:
                stale.append(str(target.relative_to(REPO_ROOT)))
        else:
            OUT_DIR.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
            print(f"wrote {target.relative_to(REPO_ROOT)}")

    if stale:
        print("API docs are out of date. Run: python3 scripts/generate_api_docs.py")
        for path in stale:
            print(f"  stale: {path}")
        return 1
    if args.check:
        print("API docs are up to date.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
