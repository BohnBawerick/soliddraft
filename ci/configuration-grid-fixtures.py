from pathlib import Path
import csv, io, json, zipfile
from xml.etree import ElementTree as E

NS = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main'
REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'
HEAD = ['configuration', 'value:plate_w [mm]', 'value:flange_d [mm]', 'value:hole_d [mm]', 'property:title [text]']
G0 = [HEAD, ['Default', '140', 'plate_w / 2', '12', 'Bracket, "family"'], ['Narrow', '120', '', '', ''], ['Wide, "shop"', '152.4 mm', '', '', 'Bracket, "wide"']]
G1 = [row[:] for row in G0]
G1[2][1] = '130 mm'

def cell_ref(row, col):
    return chr(65 + col) + str(row + 1)

def xlsx(rows, formula=False, shared_numeric=False):
    shared = []
    shared_uses = 0
    sheet = E.Element('worksheet', xmlns=NS)
    data = E.SubElement(sheet, 'sheetData')
    for r, row in enumerate(rows):
        erow = E.SubElement(data, 'row', r=str(r + 1))
        for c, value in enumerate(row):
            cell = E.SubElement(erow, 'c', r=cell_ref(r, c))
            if formula and (r, c) == (2, 1):
                E.SubElement(cell, 'f').text = '65*2'
                E.SubElement(cell, 'v').text = '130'
            elif shared_numeric and r == 1 and rows[0][c] == 'value:hole_d [mm]':
                E.SubElement(cell, 'v').text = value
            elif shared_numeric:
                cell.set('t', 's')
                if value not in shared:
                    shared.append(value)
                E.SubElement(cell, 'v').text = str(shared.index(value))
                shared_uses += 1
            else:
                cell.set('t', 'inlineStr')
                E.SubElement(E.SubElement(cell, 'is'), 't').text = value
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
        content_override = '<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>' if shared_numeric else ''
        z.writestr('[Content_Types].xml', '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>' + content_override + '</Types>')
        z.writestr('_rels/.rels', f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>')
        z.writestr('xl/workbook.xml', f'<workbook xmlns="{NS}" xmlns:r="{REL}"><sheets><sheet name="Configurations" sheetId="1" r:id="rId1"/></sheets></workbook>')
        shared_rel = f'<Relationship Id="rId2" Type="{REL}/sharedStrings" Target="sharedStrings.xml"/>' if shared_numeric else ''
        z.writestr('xl/_rels/workbook.xml.rels', f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/>{shared_rel}</Relationships>')
        if shared_numeric:
            strings = E.Element('sst', xmlns=NS, count=str(shared_uses), uniqueCount=str(len(shared)))
            for value in shared:
                E.SubElement(E.SubElement(strings, 'si'), 't').text = value
            z.writestr('xl/sharedStrings.xml', E.tostring(strings, encoding='utf-8', xml_declaration=True))
        z.writestr('xl/worksheets/sheet1.xml', E.tostring(sheet, encoding='utf-8', xml_declaration=True))
    return out.getvalue()

def csv_bytes(rows):
    out = io.StringIO(newline='')
    csv.writer(out, lineterminator='\r\n').writerows(rows)
    return out.getvalue().encode('utf-8')

def check_xlsx(blob, expected):
    with zipfile.ZipFile(io.BytesIO(blob)) as z:
        sheet = E.fromstring(z.read('xl/worksheets/sheet1.xml'))
        shared = []
        if 'xl/sharedStrings.xml' in z.namelist():
            shared = [node.findtext(f'{{{NS}}}t') or '' for node in E.fromstring(z.read('xl/sharedStrings.xml')).findall(f'{{{NS}}}si')]
        def value(cell):
            if cell.get('t') == 's':
                return shared[int(cell.findtext(f'{{{NS}}}v'))]
            if cell.get('t') == 'inlineStr':
                return cell.findtext(f'{{{NS}}}is/{{{NS}}}t') or ''
            return cell.findtext(f'{{{NS}}}v') or ''
        rows = [[value(c) for c in row] for row in sheet.findall(f'{{{NS}}}sheetData/{{{NS}}}row')]
        assert rows == expected

if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('out', type=Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    for name, rows in [('g0', G0), ('g1', G1)]:
        csv_blob, xlsx_blob = csv_bytes(rows), xlsx(rows)
        (args.out / f'{name}.csv').write_bytes(csv_blob)
        (args.out / f'{name}.xlsx').write_bytes(xlsx_blob)
        assert list(csv.reader(io.StringIO(csv_blob.decode('utf-8'), newline=''))) == rows
        check_xlsx(xlsx_blob, rows)
    inch = [row[:] for row in G1]
    inch[-1][1] = '6 in'
    (args.out / 'g1-inch.csv').write_bytes(csv_bytes(inch))
    order = [0, 3, 2, 1, 4]
    reordered = [[row[i] for i in order] for row in inch]
    (args.out / 'g1-inch.xlsx').write_bytes(xlsx(reordered, shared_numeric=True))
    check_xlsx(xlsx(reordered, shared_numeric=True), reordered)
    (args.out / 'formula.xlsx').write_bytes(xlsx(G1, formula=True))
    (args.out / 'broken.xlsx').write_bytes(b'not a ZIP archive')
    bad = [row[:] for row in G1]
    bad[-1][1] = '30 deg'
    (args.out / 'bad-unit.csv').write_bytes(csv_bytes(bad))
    (args.out / 'bad-unit.xlsx').write_bytes(xlsx(bad))
    bad = [row[:] for row in G1]
    bad[2][2], bad[2][3] = '5 mm', '200 mm'
    (args.out / 'empty-active.csv').write_bytes(csv_bytes(bad))
    (args.out / 'empty-active.xlsx').write_bytes(xlsx(bad))
    print(json.dumps({'csv_and_xlsx_match': True, 'rows_per_grid': 4, 'columns': 5, 'narrow_before': G0[2][1], 'narrow_after': G1[2][1], 'quoted_name': G1[-1][0], 'empty_active_depth_mm': 5, 'empty_active_hole_d_mm': 200}))
