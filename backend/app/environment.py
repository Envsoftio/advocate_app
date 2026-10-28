"""MatterDraft settings, with legacy deployment names accepted during upgrades."""
import os


def setting(name: str, default: str | None = None) -> str | None:
    # An explicitly empty new value must not fall back to an old credential.
    return os.environ.get('MATTERDRAFT_' + name,
                          os.environ.get('COURTBUNDLE_' + name, default))
