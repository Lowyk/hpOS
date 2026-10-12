#!/usr/bin/env python3
"""Build a BIOS disk image using Rust, Clang's integrated assembler and LLD."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / "build"
SECTOR = 512
STAGE_SECTORS = 32
KERNEL_LBA = 2048
KERNEL_BASE = 0x100000
MAX_KERNEL_BYTES = 0x2F0000
DATA_LBA = 8192
IMAGE_SECTORS = 65536
DATA_SECTORS = IMAGE_SECTORS - DATA_LBA
THUNK_BASE = 0x8000
VERSION = "0.10"
SERVICES = ["input", "display", "fs", "net"]
USER_STACK_BYTES = 256 * 4096


def largest_stack_frame(code):
    """Largest fixed frame: stack-probe `sub r11, imm32` or `sub rsp, imm32`."""
    largest = 0
    for prefix in (b"\x49\x81\xeb", b"\x48\x81\xec"):
        start = 0
        while (at := code.find(prefix, start)) >= 0:
            largest = max(largest, struct.unpack_from("<I", code, at + 3)[0] if at + 7 <= len(code) else 0)
            start = at + 1
    return largest


def elf_symbols(path):
    raw = path.read_bytes()
    h = struct.unpack_from("<HHIQQQIHHHHHH", raw, 16)
    sections = [struct.unpack_from("<IIQQQQIIQQ", raw, h[5] + i * h[10]) for i in range(h[11])]
    symbols = {}
    for section in sections:
        if section[1] != 2:
            continue
        strings = sections[section[6]]
        names = raw[strings[4]:strings[4] + strings[5]]
        for offset in range(section[4], section[4] + section[5], section[9]):
            name, _, _, _, value, _ = struct.unpack_from("<IBBHQQ", raw, offset)
            label = names[name:names.find(b"\0", name)].decode()
            symbols[label] = value
    return symbols


def run(args, **kwargs):
    return subprocess.run([str(a) for a in args], cwd=ROOT, check=True, **kwargs)


def elf_image(path, bits=64):
    """Pack PT_LOAD segments, including zero-initialized memory, without objcopy."""
    data = path.read_bytes()
    if data[:7] != b"\x7fELF" + bytes([2 if bits == 64 else 1, 1, 1]):
        raise ValueError(f"not a little-endian ELF{bits}: {path}")
    if bits == 64:
        header = struct.unpack_from("<HHIQQQIHHHHHH", data, 16)
        fmt = "<IIQQQQQQ"
    else:
        header = struct.unpack_from("<HHIIIIIHHHHHH", data, 16)
        fmt = "<IIIIIIII"
    kind, machine, _, entry, phoff = header[:5]
    if kind != 2 or machine != (62 if bits == 64 else 3):
        raise ValueError("expected a static executable for the selected x86 ISA")
    phsize, phnum = header[8:10]
    if phsize != struct.calcsize(fmt):
        raise ValueError("unexpected ELF program header size")
    segments = []
    executable = []
    for i in range(phnum):
        fields = struct.unpack_from(fmt, data, phoff + i * phsize)
        if bits == 64:
            typ, flags, offset, virtual, physical, filesz, memsz, _ = fields
        else:
            typ, offset, virtual, physical, filesz, memsz, flags, _ = fields
        if typ in (2, 3):
            raise ValueError("dynamic linking is not supported")
        if typ != 1 or memsz == 0:
            continue
        if physical != virtual or filesz > memsz or offset + filesz > len(data):
            raise ValueError("invalid load segment")
        segments.append((physical, data[offset:offset + filesz], memsz))
        if flags & 1:
            executable.append((physical, physical + memsz))
    if not segments or not any(start <= entry < end for start, end in executable):
        raise ValueError("ELF entry point is outside executable memory")
    segments.sort()
    base = segments[0][0]
    end = max(addr + size for addr, _, size in segments)
    if end - base > MAX_KERNEL_BYTES:
        raise ValueError("image exceeds bootstrap memory budget")
    output = bytearray(end - base)
    previous_end = base
    for addr, contents, size in segments:
        if addr < previous_end:
            raise ValueError("overlapping load segments")
        output[addr - base:addr - base + len(contents)] = contents
        previous_end = addr + size
    return bytes(output), base, entry


def assemble(source, address, name, lld, clang, env):
    obj = BUILD / f"{name}.o"
    elf = BUILD / f"{name}.elf"
    script = BUILD / f"{name}.ld"
    script.write_text(f"ENTRY(_start)\nPHDRS {{ code PT_LOAD FLAGS(5); }}\nSECTIONS {{ . = {address}; .text : {{ *(.text*) }} :code /DISCARD/ : {{ *(.comment*) *(.note*) }} }}\n")
    run([clang, "-target", "i386-none-elf", "-integrated-as", "-c", "-I", BUILD, source, "-o", obj])
    run([lld, "-flavor", "gnu", "-m", "elf_i386", "-T", script, "-o", elf, obj], env=env)
    binary, base, entry = elf_image(elf, 32)
    if base != address or entry != address:
        raise ValueError("unexpected bootloader load/entry address")
    return binary


def make_image(mbr, stage2, kernel, volume=b""):
    if len(mbr) > 446:
        raise ValueError("MBR overlaps the partition table")
    if len(stage2) > STAGE_SECTORS * SECTOR:
        raise ValueError("stage2 exceeds its disk slot")
    if not kernel or len(kernel) > MAX_KERNEL_BYTES:
        raise ValueError("kernel exceeds the reserved first 4 MiB")
    if len(volume) > DATA_SECTORS * SECTOR:
        raise ValueError("data volume exceeds its partition")
    image = bytearray(IMAGE_SECTORS * SECTOR)
    image[:len(mbr)] = mbr
    # Partition 1 holds the kernel; stage2 lives in the pre-partition embedding area.
    # Partition 2 (type 0x7f) is the HPFS data volume, the only area hpOS writes.
    image[446:462] = struct.pack("<B3sB3sII", 0x80, b"\xfe\xff\xff", 0xda,
                                b"\xfe\xff\xff", KERNEL_LBA, DATA_LBA - KERNEL_LBA)
    image[462:478] = struct.pack("<B3sB3sII", 0x00, b"\xfe\xff\xff", 0x7f,
                                b"\xfe\xff\xff", DATA_LBA, DATA_SECTORS)
    image[510:512] = b"\x55\xaa"
    image[SECTOR:SECTOR + len(stage2)] = stage2
    image[KERNEL_LBA * SECTOR:KERNEL_LBA * SECTOR + len(kernel)] = kernel
    image[DATA_LBA * SECTOR:DATA_LBA * SECTOR + len(volume)] = volume
    return image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    BUILD.mkdir(exist_ok=True)
    clang = shutil.which("clang")
    if not clang:
        raise SystemExit("Clang is required; no external assembler is used")
    sysroot = run(["rustc", "--print", "sysroot"], capture_output=True, text=True).stdout.strip()
    host = run(["rustc", "-vV"], capture_output=True, text=True).stdout
    host = next(line.split(": ", 1)[1] for line in host.splitlines() if line.startswith("host:"))
    lld = Path(sysroot) / "lib/rustlib" / host / "bin/rust-lld"
    if not lld.is_file():
        raise SystemExit("rust-lld was not found in the active Rust toolchain")
    target = "x86_64-unknown-none"
    if not (Path(sysroot) / "lib/rustlib" / target / "lib").is_dir():
        raise SystemExit("Install the target first: rustup target add x86_64-unknown-none")
    host_env = os.environ.copy()
    # Izzop is trained from the private corpus when it exists; the corpus never
    # enters the repository, the model only enters this locally built image.
    corpus = Path(os.environ.get("HPOS_IZZOP_CORPUS", ROOT / "private/izzop/corpus.txt"))
    izzop = []
    if corpus.is_file():
        model = BUILD / "izzop.bin"
        run(["cargo", "run", "--offline", "--release", "-q", "-p", "hpos-izzop", "--features", "std",
             "--example", "train", "--", corpus, model], env=host_env)
        izzop = [model]
    volume_path = BUILD / "volume.img"
    run(["cargo", "run", "--offline", "--release", "-q", "-p", "hpos-fs", "--example", "mkfs", "--",
         volume_path, DATA_SECTORS, *izzop], env=host_env)
    volume = volume_path.read_bytes()
    env = os.environ.copy()
    # Direct LLD invocation needs the same library search path rustc supplies.
    for key in ("DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"):
        env[key] = str(Path(sysroot) / "lib") + (os.pathsep + env[key] if env.get(key) else "")
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    env["CARGO_TARGET_X86_64_UNKNOWN_NONE_LINKER"] = str(lld)
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join([
        "-C", "relocation-model=static", "-C", "code-model=kernel", "-C", "target-cpu=x86-64",
        "-C", f"link-arg=-T{ROOT / 'services/link.ld'}", "-C", "link-arg=--gc-sections",
        "-C", "link-arg=--build-id=none"])
    thunk = assemble(ROOT / "boot/bios-thunk.S", THUNK_BASE, "thunk", lld, clang, env)
    if not 0x1000 < len(thunk) <= 0x2000:
        raise ValueError("BIOS thunk must fill one code page and fit one data page")
    (BUILD / "thunk.bin").write_bytes(thunk)
    run(["cargo", "build", "--offline", "--release", "--target", target, "-p", "hpos-services", "--bins"], env=env)
    assets = []
    service_manifest = {}
    for name in SERVICES:
        service_elf = ROOT / "target" / target / f"release/hpos-{name}"
        packed, base, entry = elf_image(service_elf)
        symbols = elf_symbols(service_elf)
        if base != 0xffffffff80000000 or entry != base or len(packed) > 0x80000:
            raise ValueError("invalid service image layout")
        text = symbols["__text_end"] - base
        ro = symbols["__rodata_end"] - base
        probe = symbols["user_kernel_probe"]
        resume = symbols["user_probe_resume"]
        if not (base < probe < resume < base + text) or not (0 < text <= ro <= 0x80000):
            raise ValueError("invalid service code boundaries")
        frame = largest_stack_frame(packed[:text])
        if frame > USER_STACK_BYTES // 2:
            raise ValueError(f"{name}: a {frame}-byte stack frame does not fit the "
                             f"{USER_STACK_BYTES}-byte user stack with headroom")
        path = BUILD / f"{name}.bin"
        path.write_bytes(packed)
        assets.append("Image { bytes: include_bytes!(" + json.dumps(str(path)) +
                      f"), executable_end: {text}, readonly_end: {ro}, probe: {probe}, resume: {resume}" + " }")
        service_manifest[name] = {"bytes": len(packed), "sha256": hashlib.sha256(packed).hexdigest(),
                                  "largest_stack_frame": frame}
    (BUILD / "services.rs").write_text(f"pub static SERVICES: [Image; {len(SERVICES)}] = [\n" + ",\n".join(assets) + "\n];\n")
    env["HPOS_BUILD_DIR"] = str(BUILD)
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join([
        "-C", "relocation-model=static", "-C", "code-model=kernel", "-C", "target-cpu=x86-64",
        "-C", f"link-arg=-T{ROOT / 'kernel/link.ld'}", "-C", "link-arg=--gc-sections",
        "-C", "link-arg=--build-id=none"])
    run(["cargo", "build", "--offline", "--release", "--target", target, "-p", "hpos-kernel"], env=env)
    elf = ROOT / "target" / target / "release/hpos-kernel"
    kernel, base, entry = elf_image(elf)
    if base != KERNEL_BASE:
        raise ValueError("kernel is not linked at 1 MiB")
    (BUILD / "kernel.bin").write_bytes(kernel)
    sectors = (len(kernel) + SECTOR - 1) // SECTOR
    (BUILD / "layout.h").write_text(
        f"#define KERNEL_LBA {KERNEL_LBA}\n#define KERNEL_SECTORS {sectors}\n"
        f"#define KERNEL_BYTES {len(kernel)}\n#define KERNEL_END {base + len(kernel)}\n"
        f"#define KERNEL_ENTRY {entry}\n")
    mbr = assemble(ROOT / "boot/mbr.S", 0x7c00, "mbr", lld, clang, env)
    stage2 = assemble(ROOT / "boot/stage2.S", 0x8000, "stage2", lld, clang, env)
    (BUILD / "mbr.bin").write_bytes(mbr)
    (BUILD / "stage2.bin").write_bytes(stage2)
    image = make_image(mbr, stage2, kernel, volume)
    (BUILD / "hpos.img").write_bytes(image)
    (BUILD / f"hpos-{VERSION}.img").write_bytes(image)
    manifest = {"format": 1, "version": VERSION, "firmware": "legacy-bios", "architecture": "x86_64",
                "image_bytes": len(image), "mbr_bytes": len(mbr), "stage2_bytes": len(stage2),
                "kernel_bytes": len(kernel), "kernel_lba": KERNEL_LBA,
                "data_lba": DATA_LBA, "data_sectors": DATA_SECTORS, "thunk_bytes": len(thunk),
                "izzop_model_from_private_corpus": bool(izzop),
                "kernel_entry": entry, "image_sha256": hashlib.sha256(image).hexdigest(),
                "boot_tested": False, "services": service_manifest,
                "font_atlas_sha256": hashlib.sha256((ROOT / "crates/desktop/assets/interface.bin").read_bytes()).hexdigest()}
    (BUILD / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Built {BUILD / 'hpos.img'} ({len(image):,} bytes); hardware boot is not yet verified.")


if __name__ == "__main__":
    main()
