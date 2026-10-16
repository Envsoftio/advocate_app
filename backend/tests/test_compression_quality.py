"""PDF-02 measured visual acceptance on synthetic scan, seal and tiny text."""
from scripts.pdf02_compression_acceptance import assert_acceptance, build_acceptance_artifacts


def test_compression_visual_quality_budgets(tmp_path):
    report = build_acceptance_artifacts(tmp_path / 'pdf02', review_sheets=False)
    assert_acceptance(report)

    assert report['fixture']['seal_status'] == 'illustrative_not_verified'
    assert set(report['presets']) == {'lossless', 'balanced', 'small'}
    assert report['presets']['lossless']['changed'] is False
    assert report['presets']['balanced']['changed'] is True
    assert report['presets']['small']['changed'] is True
