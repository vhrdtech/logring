"""Offline smoke test: the module loads and exposes a version. No card or image needed."""

import logring


def test_version():
    assert isinstance(logring.__version__, str)
    assert logring.__version__
