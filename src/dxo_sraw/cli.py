from __future__ import annotations

import argparse
import sys
from pathlib import Path

from . import tiff as t
from .superraw import SuperRawError, parse, split_file


def _cmd_info(args: argparse.Namespace) -> int:
    rc = 0
    for src in args.files:
        try:
            sr = parse(src.read_bytes())
        except (OSError, ValueError) as exc:
            print(f"{src}: {exc}", file=sys.stderr)
            rc = 1
            continue
        bps = sr.tiff.get_int(sr.primary.ifd, t.BITS_PER_SAMPLE)
        print(f"{src}: SuperRAW, {len(sr.frames)} frames, {sr.width}x{sr.height}, {bps}-bit")
        for f in sr.frames:
            print(f"  frame {f.index}: IFD @ {f.ifd.offset}, {f.nbytes} bytes @ {f.data[0][0]}")
    return rc


def _cmd_split(args: argparse.Namespace) -> int:
    rc = 0
    for src in args.files:
        out_dir = args.out_dir or src.parent
        try:
            outputs = split_file(src, out_dir, overwrite=args.force)
        except (OSError, SuperRawError, ValueError) as exc:
            print(f"{src}: {exc}", file=sys.stderr)
            rc = 1
            continue
        print(f"{src} -> " + ", ".join(str(p) for p in outputs))
    return rc


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="dxo-sraw", description="Split DxO ONE SuperRAW (.DXO) files into one DNG per frame."
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_split = sub.add_parser("split", help="write <name>_1.dng .. <name>_N.dng")
    p_split.add_argument("files", nargs="+", type=Path, metavar="FILE")
    p_split.add_argument("-o", "--out-dir", type=Path, help="output directory (default: next to input)")
    p_split.add_argument("-f", "--force", action="store_true", help="overwrite existing outputs")
    p_split.set_defaults(func=_cmd_split)

    p_info = sub.add_parser("info", help="show the frame layout of SuperRAW files")
    p_info.add_argument("files", nargs="+", type=Path, metavar="FILE")
    p_info.set_defaults(func=_cmd_info)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
