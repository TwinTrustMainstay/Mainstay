#!/usr/bin/env python3
"""Export asset JSON snapshots to CSV and/or Excel-compatible XLSX."""

import argparse
import csv
import json
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape


FIELDS = (
    "asset_id",
    "asset_type",
    "metadata",
    "serial_number",
    "owner",
    "registered_at",
    "metadata_updated_at",
    "metadata_version",
    "deprecation_status",
    "is_locked",
    "lender",
    "loan_id",
)


def read_assets(directory):
    assets = []
    for path in sorted((directory / "assets").glob("*.json")):
        with path.open(encoding="utf-8") as source:
            asset = json.load(source)
        if not isinstance(asset, dict):
            raise ValueError(f"{path} must contain a JSON object")
        assets.append({field: asset.get(field, "") for field in FIELDS})
    return assets


def write_csv(assets, destination):
    with destination.open("w", encoding="utf-8", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=FIELDS)
        writer.writeheader()
        writer.writerows(assets)


def cell(value, row, column):
    reference = f"{column}{row}"
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return f'<c r="{reference}"><v>{value}</v></c>'
    return (
        f'<c r="{reference}" t="inlineStr"><is><t xml:space="preserve">'
        f"{escape(str(value))}</t></is></c>"
    )


def write_xlsx(assets, destination):
    rows = [FIELDS] + [[asset[field] for field in FIELDS] for asset in assets]
    sheet_rows = []
    for row_number, values in enumerate(rows, start=1):
        cells = "".join(cell(value, row_number, chr(65 + index)) for index, value in enumerate(values))
        sheet_rows.append(f'<row r="{row_number}">{cells}</row>')
    worksheet = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        f"<sheetData>{''.join(sheet_rows)}</sheetData></worksheet>"
    )
    workbook = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
        '<sheets><sheet name="Assets" sheetId="1" r:id="rId1"/></sheets></workbook>'
    )
    content_types = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        '</Types>'
    )
    relationships = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
        '</Relationships>'
    )
    workbook_relationships = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
        '</Relationships>'
    )
    with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("[Content_Types].xml", content_types)
        archive.writestr("_rels/.rels", relationships)
        archive.writestr("xl/workbook.xml", workbook)
        archive.writestr("xl/_rels/workbook.xml.rels", workbook_relationships)
        archive.writestr("xl/worksheets/sheet1.xml", worksheet)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("backup_dir", type=Path)
    parser.add_argument("--format", choices=("csv", "xlsx", "both"), default="both")
    parser.add_argument("--output-dir", type=Path)
    args = parser.parse_args()

    assets = read_assets(args.backup_dir)
    output_dir = args.output_dir or args.backup_dir
    output_dir.mkdir(parents=True, exist_ok=True)
    if args.format in ("csv", "both"):
        write_csv(assets, output_dir / "assets.csv")
    if args.format in ("xlsx", "both"):
        write_xlsx(assets, output_dir / "assets.xlsx")
    print(f"Exported {len(assets)} assets to {output_dir}")


if __name__ == "__main__":
    main()
