"""Build Tesseract's in-process OCR dependency from local upstream source trees."""
import argparse
import os
from pathlib import Path
import platform
import subprocess

TARGETS = ("arm64-darwin", "x86_64-darwin", "x86_64-windows")


def build(args):
    cmake = args.cmake
    root = args.build_root.resolve()
    install = root / "install"
    options = [
        "-DCMAKE_BUILD_TYPE=Release",
        f"-DCMAKE_INSTALL_PREFIX={install}",
        "-DCMAKE_INSTALL_LIBDIR=lib",
        "-DBUILD_SHARED_LIBS=ON",
        "-DSW_BUILD=OFF",
    ]
    arch, system = args.target.rsplit("-", 1)
    if system == "darwin":
        if platform.system() != "Darwin":
            raise ValueError("Use a macOS toolchain for macOS targets")
        options.extend((f"-DCMAKE_OSX_ARCHITECTURES={arch}",
                        f"-DCMAKE_OSX_DEPLOYMENT_TARGET={args.minimum_macos}"))
        if arch != platform.machine():
            options.extend(("-DCMAKE_SYSTEM_NAME=Darwin", f"-DCMAKE_SYSTEM_PROCESSOR={arch}"))
    elif args.toolchain:
        options.append(f"-DCMAKE_TOOLCHAIN_FILE={args.toolchain.resolve(strict=True)}")
    elif platform.system() != "Windows":
        raise ValueError("Supply a CMake toolchain to cross-build Windows DLLs")
    else:
        options.extend(("-A", "x64"))

    components = [
        (args.leptonica, "leptonica", [
            "-DBUILD_PROG=OFF",
            *(f"-DENABLE_{codec}=OFF" for codec in ("ZLIB", "PNG", "JPEG", "TIFF", "GIF", "WEBP", "OPENJPEG")),
        ]),
        (args.tesseract, "tesseract", [
            f"-DCMAKE_PREFIX_PATH={install}",
            "-DBUILD_TRAINING_TOOLS=OFF",
            "-DBUILD_TESTS=OFF",
            "-DENABLE_NATIVE=OFF",
            "-DOPENMP_BUILD=OFF",
            "-DGRAPHICS_DISABLED=ON",
            "-DDISABLE_TIFF=ON",
            "-DLEPT_TIFF_RESULT=1",
            "-DDISABLE_ARCHIVE=ON",
            "-DDISABLE_CURL=ON",
        ]),
    ]
    # OCR receives decoded RGB pixels from Rust. External image codecs,
    # remote input and command-line training tools are unnecessary.
    for source, name, switches in components:
        destination = root / name
        subprocess.run([cmake, "-S", str(source.resolve(strict=True)), "-B", str(destination),
                        *options, *switches], check=True)
        subprocess.run([cmake, "--build", str(destination), "--config", "Release",
                        "--parallel", str(args.jobs)], check=True)
        subprocess.run([cmake, "--install", str(destination), "--config", "Release"], check=True)
    print(f"Built {args.target} OCR libraries under {install}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--leptonica", type=Path, required=True)
    parser.add_argument("--tesseract", type=Path, required=True)
    parser.add_argument("--build-root", type=Path, required=True)
    parser.add_argument("--cmake", default="cmake")
    parser.add_argument("--toolchain", type=Path)
    parser.add_argument("--minimum-macos", default="12.0")
    parser.add_argument("--jobs", type=int, default=min(6, os.cpu_count() or 1))
    args = parser.parse_args()
    try:
        build(args)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"OCR build failed: {error}\n")


if __name__ == "__main__":
    main()
