"""Offline smoke test: the module loads and exposes a version. No card or image needed."""

import ww_logring


def test_version():
    assert isinstance(ww_logring.__version__, str)
    assert ww_logring.__version__
