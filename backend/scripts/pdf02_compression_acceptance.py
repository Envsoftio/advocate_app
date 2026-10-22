"""Generate and measure the synthetic PDF-02 visual-quality corpus.

Run from ``backend``::

    .venv312/bin/python scripts/pdf02_compression_acceptance.py \
        --output ../artifacts/pdf02-compression

The corpus contains no real case material.  Its red seal is an illustration,
not a signature or an authenticity fixture.  Automated measurements are a
regression floor; the generated review sheets remain the human-review input.
"""
from __future__ import annotations

import argparse
import io
import json
import math
from pathlib import Path
import random
import shutil
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFont, ImageStat
import pymupdf

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from app.pdf_ops import compress_pdf


SOURCE_DPI = 300
PAGE_POINTS = (595, 842)
PAGE_PIXELS = (2480, 3508)
PAGE_NAMES = ('scan', 'seal', 'small-text')
REGIONS = {
    'scan': (150, 310, 2330, 3240),
    'seal': (500, 600, 2020, 2220),
    'small-text': (145, 310, 2335, 3220),
}

# These budgets are deliberately preset-specific.  They describe this fixed
# synthetic corpus, not a promise for arbitrary filings or portal size limits.
QUALITY_BUDGETS = {
    'lossless': {
        'max_size_ratio': 1.0,
        'scan': {'min_psnr_db': 99.0},
        'seal': {'min_psnr_db': 99.0},
        'small-text': {'min_psnr_db': 99.0},
    },
    'balanced': {
        'max_size_ratio': 0.20,
        'scan': {'min_psnr_db': 28.0, 'min_ink_recall': 0.97,
                 'min_contrast_retention': 0.75},
        'seal': {'min_psnr_db': 27.0, 'min_red_recall': 0.94,
                 'min_red_precision': 0.88},
        'small-text': {'min_psnr_db': 25.8, 'min_ink_recall': 0.97,
                       'min_contrast_retention': 0.70},
    },
    'small': {
        'max_size_ratio': 0.08,
        'scan': {'min_psnr_db': 24.0, 'min_ink_recall': 0.88,
                 'min_contrast_retention': 0.65},
        'seal': {'min_psnr_db': 23.0, 'min_red_recall': 0.86,
                 'min_red_precision': 0.78},
        'small-text': {'min_psnr_db': 21.5, 'min_ink_recall': 0.78,
                       'min_contrast_retention': 0.58},
    },
}


def _font(size: int):
    # Pillow ships this scalable font, avoiding host-font differences.
    return ImageFont.load_default(size=size)


def _paper(seed: int) -> Image.Image:
    """Return deterministic off-white scanner grain without NumPy."""
    width, height = PAGE_PIXELS
    noise = Image.frombytes('L', (width, height), random.Random(seed).randbytes(width * height))
    noise = noise.point([238 + value % 13 for value in range(256)])
    return Image.merge('RGB', (noise, noise, noise))


def _header(draw: ImageDraw.ImageDraw, title: str, subtitle: str):
    draw.text((155, 115), title, font=_font(50), fill=(28, 30, 31), stroke_width=1)
    draw.text((155, 190), subtitle, font=_font(25), fill=(75, 76, 77))
    draw.line((155, 245, 2325, 245), fill=(70, 72, 74), width=4)


def _scan_page() -> Image.Image:
    image = _paper(20260201)
    draw = ImageDraw.Draw(image)
    _header(draw, 'SYNTHETIC SCAN QUALITY FIXTURE',
            'Generated evidence only - no real matter or verified text')
    body = _font(31)
    small = _font(25)
    lines = [
        '1. This page simulates a clean office scan with light paper grain.',
        '2. Dates, amounts and references must remain visually distinguishable.',
        '3. Reference: CB-PDF02-2026 / Amount: INR 12,345.67 / Page 1 of 3.',
        '4. Compression may reduce pixels; it must not be described as lossless.',
    ]
    y = 355
    for line in lines:
        draw.text((175, y), line, font=body, fill=(45, 46, 47))
        y += 92
    for row in range(19):
        text = (f'{row + 1:02}. Synthetic paragraph line: alpha beta gamma 0123456789; '
                'review the working copy before export.')
        draw.text((175, 790 + row * 104), text, font=small,
                  fill=(58 + row % 3 * 4,) * 3)
    # Faint scanner bands and margin marks exercise low-contrast retention.
    for y in range(720, 3210, 310):
        draw.line((150, y, 2330, y + 3), fill=(215, 216, 216), width=3)
    draw.rectangle((150, 310, 2330, 3240), outline=(92, 94, 95), width=3)
    return image


def _seal_page() -> Image.Image:
    image = _paper(20260202)
    draw = ImageDraw.Draw(image)
    _header(draw, 'SYNTHETIC SEAL DETAIL FIXTURE',
            'The mark below is illustrative and does not verify a signer or document')
    body = _font(28)
    for row in range(18):
        draw.text((165, 360 + row * 145),
                  f'Background record {row + 1:02}: SAMPLE ONLY  ABC-987654  16 OCT 2026',
                  font=body, fill=(80, 82, 83))
    red = (164, 31, 38)
    cx, cy = 1260, 1410
    for radius, width in ((690, 18), (620, 9), (430, 12)):
        draw.ellipse((cx - radius, cy - radius, cx + radius, cy + radius),
                     outline=red, width=width)
    for angle in range(0, 360, 15):
        radians = math.radians(angle)
        start = (cx + int(455 * math.cos(radians)), cy + int(455 * math.sin(radians)))
        end = (cx + int(585 * math.cos(radians)), cy + int(585 * math.sin(radians)))
        draw.line((*start, *end), fill=red, width=8)
    draw.text((850, 1205), 'SYNTHETIC', font=_font(58), fill=red, stroke_width=1)
    draw.text((1040, 1390), 'SEAL', font=_font(82), fill=red, stroke_width=2)
    draw.text((865, 1595), 'NOT VERIFIED', font=_font(55), fill=red, stroke_width=1)
    draw.line((770, 1360, 1750, 1360), fill=red, width=9)
    draw.line((770, 1535, 1750, 1535), fill=red, width=9)
    return image


def _small_text_page() -> Image.Image:
    image = _paper(20260203)
    draw = ImageDraw.Draw(image)
    _header(draw, 'SMALL-TEXT LEGIBILITY FIXTURE',
            'Fine raster text and table rules - inspect at 100% after compression')
    x0, x1, x2, x3 = 155, 650, 1520, 2325
    y0, row_height = 325, 70
    draw.rectangle((x0, y0, x3, y0 + 40 * row_height), outline=(45, 46, 47), width=3)
    for x in (x1, x2):
        draw.line((x, y0, x, y0 + 40 * row_height), fill=(75, 76, 77), width=2)
    for row in range(1, 40):
        y = y0 + row * row_height
        draw.line((x0, y, x3, y), fill=(135, 136, 137), width=2)
    tiny = _font(21)   # about 5 pt at the 300 DPI source raster
    small = _font(24)  # about 5.8 pt
    for row in range(40):
        y = y0 + row * row_height + 20
        draw.text((x0 + 14, y), f'{row + 1:03}', font=small, fill=(38, 39, 40))
        draw.text((x1 + 14, y),
                  f'Clause {row + 1:02}. Ref CB-{2026 + row}-{row * 17:04}.',
                  font=tiny, fill=(42, 43, 44))
        draw.text((x2 + 14, y),
                  f'INR {row * 913 + 12_345:,.2f} / 0O1Il5S8B',
                  font=tiny, fill=(42, 43, 44))
    return image


def create_fixture_pdf(path: Path) -> None:
    pages = (_scan_page(), _seal_page(), _small_text_page())
    with pymupdf.open() as pdf:
        for image in pages:
            page = pdf.new_page(width=PAGE_POINTS[0], height=PAGE_POINTS[1])
            stream = io.BytesIO()
            image.save(stream, format='PNG', compress_level=6)
            page.insert_image(page.rect, stream=stream.getvalue())
        pdf.set_metadata({'title': 'Synthetic PDF-02 compression acceptance corpus',
                          'subject': 'No real matter; seal is illustrative and unverified'})
        pdf.set_toc([[1, 'Synthetic scan', 1], [1, 'Synthetic seal', 2],
                     [1, 'Synthetic small text', 3]])
        pdf.save(path, garbage=4, deflate=True)


def _render(path: Path) -> list[Image.Image]:
    images = []
    with pymupdf.open(path) as pdf:
        for page in pdf:
            pix = page.get_pixmap(dpi=SOURCE_DPI, colorspace=pymupdf.csRGB, alpha=False,
                                  annots=False)
            images.append(Image.frombytes('RGB', (pix.width, pix.height), pix.samples))
    return images


def _psnr(reference: Image.Image, output: Image.Image) -> float:
    stat = ImageStat.Stat(ImageChops.difference(reference, output))
    mse = sum(value * value for value in stat.rms) / len(stat.rms)
    return 99.0 if mse == 0 else 20 * math.log10(255 / math.sqrt(mse))


def _threshold(image: Image.Image, predicate) -> Image.Image:
    return image.point([255 if predicate(value) else 0 for value in range(256)]).convert('1')


def _mask_count(mask: Image.Image) -> int:
    return mask.histogram()[255]


def _precision_recall(reference: Image.Image, output: Image.Image) -> tuple[float, float]:
    intersection = ImageChops.logical_and(reference, output)
    true_positive = _mask_count(intersection)
    reference_count = _mask_count(reference)
    output_count = _mask_count(output)
    return (true_positive / max(1, output_count), true_positive / max(1, reference_count))


def _ink_metrics(reference: Image.Image, output: Image.Image) -> dict:
    reference_luma = reference.convert('L')
    output_luma = output.convert('L')
    reference_ink = _threshold(reference_luma, lambda value: value < 175)
    output_ink = _threshold(output_luma, lambda value: value < 200)
    _, recall = _precision_recall(reference_ink, output_ink)
    background = _threshold(reference_luma, lambda value: value > 225)
    reference_ink_luma = ImageStat.Stat(reference_luma, mask=reference_ink.convert('L')).mean[0]
    reference_background = ImageStat.Stat(reference_luma, mask=background.convert('L')).mean[0]
    output_ink_luma = ImageStat.Stat(output_luma, mask=reference_ink.convert('L')).mean[0]
    output_background = ImageStat.Stat(output_luma, mask=background.convert('L')).mean[0]
    reference_contrast = reference_background - reference_ink_luma
    output_contrast = output_background - output_ink_luma
    return {'ink_recall': round(recall, 4),
            'contrast_retention': round(output_contrast / max(1, reference_contrast), 4)}


def _red_mask(image: Image.Image) -> Image.Image:
    red, green, blue = image.split()
    red_high = _threshold(red, lambda value: value > 105)
    green_low = _threshold(green, lambda value: value < 150)
    blue_low = _threshold(blue, lambda value: value < 150)
    return ImageChops.logical_and(red_high, ImageChops.logical_and(green_low, blue_low))


def _page_metrics(name: str, reference: Image.Image, output: Image.Image) -> dict:
    crop = REGIONS[name]
    reference = reference.crop(crop)
    output = output.crop(crop)
    metrics = {'psnr_db': round(_psnr(reference, output), 2)}
    if name == 'seal':
        precision, recall = _precision_recall(_red_mask(reference), _red_mask(output))
        metrics.update(red_precision=round(precision, 4), red_recall=round(recall, 4))
    else:
        metrics.update(_ink_metrics(reference, output))
    return metrics


def _review_sheet(name: str, preset: str, reference: Image.Image,
                  output: Image.Image, destination: Path) -> None:
    reference = reference.crop(REGIONS[name])
    output = output.crop(REGIONS[name])
    scale = min(1.0, 1100 / reference.height)
    size = (round(reference.width * scale), round(reference.height * scale))
    reference = reference.resize(size, Image.Resampling.LANCZOS)
    output = output.resize(size, Image.Resampling.LANCZOS)
    sheet = Image.new('RGB', (size[0] * 2 + 60, size[1] + 90), 'white')
    draw = ImageDraw.Draw(sheet)
    draw.text((20, 18), f'SOURCE - {name}', font=_font(24), fill='black')
    draw.text((size[0] + 40, 18), f'{preset.upper()} - {name}',
              font=_font(24), fill='black')
    sheet.paste(reference, (20, 65))
    sheet.paste(output, (size[0] + 40, 65))
    sheet.save(destination, format='PNG', optimize=True)


def build_acceptance_artifacts(output_dir: Path, *, review_sheets: bool = True) -> dict:
    output_dir.mkdir(parents=True, exist_ok=True)
    source_path = output_dir / 'source.pdf'
    create_fixture_pdf(source_path)
    source_renders = _render(source_path)
    report = {
        'schema': 1,
        'fixture': {
            'synthetic': True,
            'source_dpi': SOURCE_DPI,
            'render_dpi': SOURCE_DPI,
            'pages': list(PAGE_NAMES),
            'seal_status': 'illustrative_not_verified',
        },
        'presets': {},
    }
    for preset in QUALITY_BUDGETS:
        compressed_path = output_dir / f'{preset}.pdf'
        shutil.copy2(source_path, compressed_path)
        compression = compress_pdf(compressed_path, preset)
        output_renders = _render(compressed_path)
        page_metrics = {
            name: _page_metrics(name, source_renders[index], output_renders[index])
            for index, name in enumerate(PAGE_NAMES)
        }
        report['presets'][preset] = {
            'before_bytes': compression['before_bytes'],
            'after_bytes': compression['after_bytes'],
            'size_ratio': round(compression['after_bytes'] / compression['before_bytes'], 4),
            'changed': compression['changed'],
            'message': compression['message'],
            'images_rewritten': compression['images_rewritten'],
            'pages': page_metrics,
            'budgets': QUALITY_BUDGETS[preset],
        }
        if review_sheets:
            for index, name in enumerate(PAGE_NAMES):
                _review_sheet(name, preset, source_renders[index], output_renders[index],
                              output_dir / f'{preset}-{name}-review.png')
    (output_dir / 'metrics.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    return report


def assert_acceptance(report: dict) -> None:
    for preset, result in report['presets'].items():
        budgets = QUALITY_BUDGETS[preset]
        assert result['size_ratio'] <= budgets['max_size_ratio'], result
        expected_rewrites = 0 if preset == 'lossless' else len(PAGE_NAMES)
        assert result['images_rewritten'] == expected_rewrites, result
        # MuPDF can save a few bytes in lossless PDF metadata serialization.
        # Assert the actual size/result contract, not an incidental byte count.
        assert result['changed'] is (result['after_bytes'] < result['before_bytes']), result
        if preset != 'lossless':
            assert result['changed'], result
        if not result['changed']:
            assert result['after_bytes'] == result['before_bytes'], result
            assert result['message'] == 'No smaller result; working copy retained.'
        else:
            assert result['message'] == 'Compressed working copy; review visual quality.'
        for name, metrics in result['pages'].items():
            budget = budgets[name]
            for key, minimum in budget.items():
                metric = key.removeprefix('min_')
                assert metrics[metric] >= minimum, (preset, name, metric, metrics, budget)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True,
                        help='Destination for PDFs, PNG review sheets and metrics.json')
    args = parser.parse_args()
    report = build_acceptance_artifacts(args.output)
    assert_acceptance(report)
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
