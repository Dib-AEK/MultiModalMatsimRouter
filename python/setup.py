"""Marks ctypes distributions containing native libraries as platform-specific Python wheels."""

from pathlib import Path
from setuptools import setup
from wheel.bdist_wheel import bdist_wheel


class NativeWheel(bdist_wheel):
    def finalize_options(self):
        super().finalize_options()
        self.root_is_pure = False

    def get_tag(self):
        # ctypes uses the C ABI, not CPython's extension ABI; the OS/architecture still matter.
        return "py3", "none", super().get_tag()[2]


native = Path(__file__).parent / "src/multimodal_router/_native"
setup(cmdclass={"bdist_wheel": NativeWheel} if native.is_dir() else {})
