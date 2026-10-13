"""Make synthetic, non-confidential sample documents for testing."""
from pathlib import Path
import pymupdf
from PIL import Image, ImageDraw, ImageFont

TARGET = Path(__file__).resolve().parents[2] / 'samples'
TARGET.mkdir(parents=True, exist_ok=True)


def text_pdf(filename, heading, lines, count):
    with pymupdf.open() as doc:
        for index in range(count):
            page = doc.new_page(width=595, height=842)
            page.insert_text((65, 80), heading, fontsize=18, fontname='hebo')
            page.insert_text((65, 107), f'Synthetic sample - page {index+1}/{count}', fontsize=11)
            for y, line in enumerate(lines):
                page.insert_text((65, 170 + y*34), line, fontsize=11)
            page.insert_text((65, 770), 'TEST DATA ONLY - NO REAL PARTIES OR CASES', fontsize=9, color=(.7,.2,.2))
        doc.save(TARGET / filename)


def main():
    text_pdf('01_petition.pdf', 'DRAFT PETITION - EXAMPLE ONLY', [
        'IN THE COURT (DEMONSTRATION)', 'Petitioner: Fictional Applicant',
        'Respondent: Fictional Respondent', 'This document contains no actual legal pleading.',
        'Purpose: test index, collation and bookmarking.'], 3)
    text_pdf('02_impugned_order.pdf', 'SAMPLE ORDER', [
        'Mock order dated 10 January 2026', 'This order is entirely fictitious.',
        'Used solely to demonstrate bundle preparation.'], 2)
    text_pdf('03_affidavit.pdf', 'DRAFT AFFIDAVIT - SAMPLE', [
        'I, Example Deponent, demonstrate synthetic text.',
        'This is not an executed affidavit.', 'Signature: NOT PROVIDED'], 2)
    img = Image.new('RGB', (1654, 2339), 'white')
    draw = ImageDraw.Draw(img)
    try:
        font = ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf', 44)
    except OSError:
        font = ImageFont.load_default()
    draw.text((125, 220), 'ANNEXURE P-1 (SYNTHETIC SCAN)', fill='black', font=font)
    draw.text((125, 390), 'OCR TEST DOCUMENT - 2026', fill='black', font=font)
    draw.text((125, 560), 'The quick brown fox jumps over the lazy dog.', fill='black', font=font)
    draw.rectangle((100,100,1510,2110), outline=(45,45,45), width=4)
    img.save(TARGET/'04_scanned_annexure.jpg', quality=86, dpi=(200,200))
    print('Generated sample PDFs and scanned JPEG in', TARGET)


if __name__ == '__main__':
    main()
