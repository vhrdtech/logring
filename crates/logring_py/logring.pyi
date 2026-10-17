"""Type stubs for the logring Python module."""

from typing import Any, TypedDict

__version__: str

class Session(TypedDict):
    session: int
    device_id: str
    fw: str
    hw: str
    bundle_hash: str

class Row(TypedDict):
    pos: int
    t_ps: int | None
    value: Any

class MarkerDict(TypedDict):
    pos: int
    t_ps: int | None
    kind: int
    label: str
    value: int | None

class RecordingDict(TypedDict):
    id: int
    trigger: int
    start_pos: int
    start_t_ps: int | None
    stop_pos: int | None
    stop_t_ps: int | None
    stop_reason: int | None

class Track:
    name: str
    path: list[int]
    resolved: bool
    decode_errors: int
    key: str
    def __len__(self) -> int: ...
    def rows(self) -> list[Row]: ...
    def to_numpy(self) -> dict[str, Any]: ...  # values are numpy.ndarray
    def to_pandas(self) -> Any: ...  # a pandas.DataFrame

class LogFile:
    def tracks(self) -> list[Track]: ...
    def track(self, name: str) -> Track: ...
    def sessions(self) -> list[Session]: ...
    def markers(self) -> list[MarkerDict]: ...
    def recordings(self) -> list[RecordingDict]: ...

def open(path: str) -> LogFile: ...
