"""Read-only search extraction selection, independent of endpoint matching rules."""
from contextlib import contextmanager
import os

from .file_io import open_pdf
from .oxide_pdf import stream_pages


@contextmanager
def search_pages(path):
    # Separate from Office opt-in: no implicit expansion of migration scope.
    if os.environ.get('MATTERDRAFT_PDF_OXIDE_SEARCH') == '1':
        with stream_pages(path) as pages:
            yield pages
    else:
        with open_pdf(path) as pdf:
            yield (page.get_text('text') for page in pdf)
