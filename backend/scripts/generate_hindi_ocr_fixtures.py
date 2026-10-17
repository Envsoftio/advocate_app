"""Generate disposable Hindi OCR samples; no case material is used."""
from io import BytesIO
from pathlib import Path
import pymupdf
from PIL import Image, ImageDraw, ImageFont, ImageEnhance, ImageFilter

TARGET = Path(__file__).resolve().parents[2] / 'samples'
FONTS = (
    Path('/System/Library/Fonts/Supplemental/Devanagari Sangam MN.ttc'),
    Path('/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf'),
    Path('/usr/share/fonts/truetype/noto/NotoSansDevanagariUI-Regular.ttf'),
)


def main():
    font_path = next((path for path in FONTS if path.is_file()), None)
    if font_path is None:
        raise SystemExit('Install a Devanagari font or use the checked-in fixtures.')
    hindi = ImageFont.truetype(str(font_path), 94)
    latin = ImageFont.truetype('/System/Library/Fonts/Supplemental/Arial.ttf', 68) if Path(
        '/System/Library/Fonts/Supplemental/Arial.ttf').is_file() else ImageFont.truetype(
        '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf', 68)
    image = Image.new('RGB', (1800, 600), 'white')
    draw = ImageDraw.Draw(image)
    draw.text((90, 60), 'हिंदी परीक्षण दस्तावेज', font=hindi, fill='black', language='hi')
    draw.text((90, 250), 'CASE 2026', font=latin, fill='black')
    buffer = BytesIO()
    image.save(buffer, format='JPEG', quality=92)
    TARGET.mkdir(exist_ok=True)
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=595, height=842)
        page.insert_image(pymupdf.Rect(35, 100, 555, 275), stream=buffer.getvalue())
        pdf.save(TARGET / '05_hindi_scan.pdf')
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=595, height=842)
        page.insert_text((45, 55), 'Native Header 2026', fontsize=18)
        page.insert_image(pymupdf.Rect(35, 120, 555, 295), stream=buffer.getvalue())
        page.set_rotation(90)
        pdf.new_page(width=595, height=842).insert_text((45, 55), 'Native page two', fontsize=18)
        pdf.save(TARGET / '06_hindi_mixed.pdf')
    degraded = image.resize((900, 300)).filter(ImageFilter.GaussianBlur(0.35))
    degraded = ImageEnhance.Contrast(degraded).enhance(0.48)
    poor = BytesIO()
    degraded.save(poor, format='JPEG', quality=55)
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=595, height=842)
        page.insert_image(pymupdf.Rect(35, 100, 555, 275), stream=poor.getvalue())
        pdf.save(TARGET / '07_hindi_poor_scan.pdf')


if __name__ == '__main__':
    main()
