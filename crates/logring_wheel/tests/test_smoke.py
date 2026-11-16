"""Offline smoke tests: the module loads (no card or image needed), then opens an image `cargo run -p
logring_cli -- make-image` built, and exercises tracks, markers, recordings, to_numpy() and to_pandas()."""

import subprocess
from pathlib import Path

import logring
import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]


def test_version():
    assert isinstance(logring.__version__, str)
    assert logring.__version__


@pytest.fixture(scope="module")
def demo_image(tmp_path_factory):
    image = tmp_path_factory.mktemp("logring_py") / "demo.wlr"
    subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "logring_cli",
            "--",
            "make-image",
            str(image),
            "--tracks",
            "2",
            "--records",
            "30",
        ],
        cwd=REPO_ROOT,
        check=True,
    )
    return image


def test_open_lists_sessions_and_tracks(demo_image):
    log = logring.open(str(demo_image))
    sessions = log.sessions()
    assert len(sessions) == 1
    assert sessions[0]["device_id"] == "make-image"

    tracks = log.tracks()
    names = sorted(t.name for t in tracks)
    assert names == ["sensor_0", "sensor_1"]
    for t in tracks:
        assert t.resolved
        assert t.decode_errors == 0
        assert len(t) == 30


def test_track_rows_and_key(demo_image):
    log = logring.open(str(demo_image))
    t = log.track("sensor_0")
    rows = t.rows()
    assert len(rows) == 30
    assert rows[0]["pos"] == 0
    assert isinstance(rows[0]["value"], float)
    assert len(t.key) == 16  # hex


def test_unknown_track_raises_key_error(demo_image):
    log = logring.open(str(demo_image))
    with pytest.raises(KeyError):
        log.track("no-such-track")


def test_to_numpy(demo_image):
    numpy = pytest.importorskip("numpy")
    log = logring.open(str(demo_image))
    arrays = log.track("sensor_0").to_numpy()
    assert set(arrays.keys()) >= {"pos", "t_ps", "value"}
    assert isinstance(arrays["value"], numpy.ndarray)
    assert arrays["value"].shape == (30,)


def test_to_pandas(demo_image):
    pytest.importorskip("pandas")
    log = logring.open(str(demo_image))
    df = log.track("sensor_0").to_pandas()
    assert len(df) == 30
    assert "value" in df.columns


def test_markers_and_recordings_are_empty_without_any(demo_image):
    log = logring.open(str(demo_image))
    assert log.markers() == []
    assert log.recordings() == []


def test_mf4_export_opens_in_asammdf(demo_image, tmp_path):
    """R10's "done when": `logring export --format mf4`'s output opens in `asammdf`, not just by hand."""
    asammdf = pytest.importorskip("asammdf")
    mf4 = tmp_path / "sensor_0.mf4"
    subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "logring_cli",
            "--",
            "export",
            str(demo_image),
            "--track",
            "sensor_0",
            "--format",
            "mf4",
            "-o",
            str(mf4),
        ],
        cwd=REPO_ROOT,
        check=True,
    )
    mdf = asammdf.MDF(str(mf4))
    try:
        signal = mdf.get("value")
        assert len(signal.samples) == 30
    finally:
        mdf.close()
