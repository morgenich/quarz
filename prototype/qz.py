import argparse
import ctypes
import hashlib
import json
import lzma
import os
import re
import struct
import sys
import threading
import time
import zlib
from collections import OrderedDict, deque
from concurrent.futures import ThreadPoolExecutor

MAGIC = b"QZ1\n"
END_MAGIC = b"QZ1E"
FOOTER = struct.Struct("<QQ4s")

MAX_DEPTH = 4
MIN_BLOB = 64
MIN_COMP = 32
MAX_RAW = 1 << 30
BIG_FILE = 256 << 20
PIECE = 64 << 20
WINDOW_BYTES = 128 << 20

CHUNK_MIN = 2048
CHUNK_MAX = 65536
CHUNKED_FROM = 32768
SEGMENT = 1 << 20

K_RAWDEFLATE, K_ZLIB, K_PNG = 0, 1, 2

LEVELS = {
    "fast": dict(preset=1, dict_size=4 << 20, block=8 << 20, store_above=0.95),
    "normal": dict(preset=6, dict_size=16 << 20, block=32 << 20, store_above=0.995),
    "max": dict(preset=9 | lzma.PRESET_EXTREME, dict_size=64 << 20, block=128 << 20, store_above=0.999),
}

def _anchor_pattern():
    sets = []
    for mult, add, size in ((167, 13, 16), (101, 7, 16), (59, 31, 8)):
        members = bytes(b for b in range(256) if (b * mult + add) % 256 < size)
        sets.append(b"[" + b"".join(re.escape(bytes([b])) for b in members) + b"]")
    return re.compile(b"".join(sets), re.DOTALL)

ANCHOR_RE = _anchor_pattern()

def cut_points(data):
    cuts, last, n = [], 0, len(data)
    for match in ANCHOR_RE.finditer(data):
        end = match.end()
        while end - last > CHUNK_MAX:
            last += CHUNK_MAX
            cuts.append(last)
        if end - last >= CHUNK_MIN:
            cuts.append(end)
            last = end
    while n - last > CHUNK_MAX:
        last += CHUNK_MAX
        cuts.append(last)
    if last < n:
        cuts.append(n)
    return cuts

def digest(data):
    return hashlib.blake2b(data, digest_size=16).digest()

def long_path(path):
    path = os.path.abspath(path)
    if os.name != "nt" or path.startswith("\\\\?\\"):
        return path
    if path.startswith("\\\\"):
        return "\\\\?\\UNC\\" + path[2:]
    return "\\\\?\\" + path

def total_ram():
    try:
        if os.name == "nt":
            class Status(ctypes.Structure):
                _fields_ = [("length", ctypes.c_ulong), ("load", ctypes.c_ulong)] + [
                    (name, ctypes.c_ulonglong) for name in
                    ("total", "avail", "total_page", "avail_page", "total_virt", "avail_virt", "ext")]
            status = Status()
            status.length = ctypes.sizeof(Status)
            ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status))
            return status.total
        return os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    except Exception:
        return 8 << 30

def deflate_pieces(raw, wbits, level, mem, strategy):
    co = zlib.compressobj(level, zlib.DEFLATED, wbits, mem, strategy)
    view = memoryview(raw)
    pos, step, n = 0, 1 << 16, len(raw)
    while pos < n:
        out = co.compress(view[pos:pos + step])
        pos += step
        if step < 1 << 22:
            step <<= 1
        if out:
            yield out
    yield co.flush()

def reproduces(raw, comp, params):
    view = memoryview(comp)
    pos = 0
    try:
        for out in deflate_pieces(raw, *params):
            end = pos + len(out)
            if view[pos:end] != out:
                return False
            pos = end
    except (ValueError, zlib.error):
        return False
    return pos == len(comp)

def _candidates():
    out = []
    for strategy in (0, 1):
        for mem in (8, 9):
            for level in (6, 9, 1, 5, 7, 8, 4, 3, 2):
                out.append((level, mem, strategy))
    for strategy in (3, 2, 4):
        for mem in (8, 9):
            out.append((6, mem, strategy))
    out.append((0, 8, 0))
    return out

CANDIDATES = _candidates()

def _flevel(level, strategy):
    if strategy >= 2 or level < 2:
        return 0
    if level < 6:
        return 1
    return 2 if level == 6 else 3

class DeflateFinder:

    def __init__(self):
        self.recent = []
        self.lock = threading.Lock()

    def find(self, raw, comp, wbits):
        flevel = comp[1] >> 6 if wbits > 0 else None
        tried = set()
        for cand in self.recent[:] + CANDIDATES:
            if cand in tried:
                continue
            tried.add(cand)
            if flevel is not None and _flevel(cand[0], cand[2]) != flevel:
                continue
            params = (wbits,) + cand
            if reproduces(raw, comp, params):
                with self.lock:
                    if cand in self.recent:
                        self.recent.remove(cand)
                    self.recent.insert(0, cand)
                    del self.recent[4:]
                return params
        return None

PNG_SIG = b"\x89PNG\r\n\x1a\n"

def _scan_pattern():
    alts = [re.escape(b"PK\x03\x04"), re.escape(b"\x1f\x8b\x08"), re.escape(PNG_SIG)]
    for cinfo in range(1, 8):
        cmf = (cinfo << 4) | 8
        flags = [f for f in range(256) if not f & 0x20 and (cmf * 256 + f) % 31 == 0]
        alts.append(re.escape(bytes([cmf])) + b"[" + b"".join(re.escape(bytes([f])) for f in flags) + b"]")
    return re.compile(b"|".join(alts), re.DOTALL)

SCAN_RE = _scan_pattern()

def _inflate(buf, start, wbits, waste):
    d = zlib.decompressobj(wbits)
    parts, total, pos, step, n = [], 0, start, 1024, len(buf)
    try:
        while not d.eof:
            if pos >= n:
                waste[0] += total
                return None
            chunk = buf[pos:pos + step]
            pos += len(chunk)
            while chunk and not d.eof:
                out = d.decompress(chunk, 1 << 24)
                total += len(out)
                if total > MAX_RAW:
                    parts = None
                elif parts is not None:
                    parts.append(out)
                chunk = d.unconsumed_tail
            step = min(step * 4, 1 << 20)
    except zlib.error:
        waste[0] += total
        return None
    end = pos - len(d.unused_data)
    if parts is None:
        waste[0] += total
        return None, end
    return b"".join(parts), end

def _stream(buf, start, end, kind, wbits, raw, ctx, attempt=True):
    if raw is None or not attempt or end - start < MIN_COMP or len(raw) < MIN_BLOB:
        return ("skip", end)
    params = ctx.finder.find(raw, buf[start:end], wbits)
    if params is None:
        return ("skip", end)
    return (start, end, kind, params, None, raw)

def _zip_entry(buf, at, ctx, attempt, waste):
    if at + 30 > len(buf):
        return None
    flags, method = struct.unpack_from("<HH", buf, at + 6)
    if method != 8 or flags & 1:
        return None
    name_len, extra_len = struct.unpack_from("<HH", buf, at + 26)
    start = at + 30 + name_len + extra_len
    res = _inflate(buf, start, -15, waste)
    if res is None:
        return None
    return _stream(buf, start, res[1], K_RAWDEFLATE, -15, res[0], ctx, attempt)

def _gzip_member(buf, at, ctx, attempt, waste):
    n = len(buf)
    if at + 18 > n:
        return None
    flags = buf[at + 3]
    if flags & 0xE0:
        return None
    pos = at + 10
    if flags & 4:
        if pos + 2 > n:
            return None
        pos += 2 + struct.unpack_from("<H", buf, pos)[0]
    for bit in (8, 16):
        if flags & bit:
            end = buf.find(b"\0", pos, pos + 65536)
            if end == -1:
                return None
            pos = end + 1
    if flags & 2:
        pos += 2
    res = _inflate(buf, pos, -15, waste)
    if res is None:
        return None
    return _stream(buf, pos, res[1], K_RAWDEFLATE, -15, res[0], ctx, attempt)

def _zlib_stream(buf, at, ctx, attempt, waste):
    res = _inflate(buf, at, 15, waste)
    if res is None:
        return None
    return _stream(buf, at, res[1], K_ZLIB, (buf[at] >> 4) + 8, res[0], ctx, attempt)

def _png(buf, at, ctx, attempt, waste):
    n = len(buf)
    pos = at + 8
    idat = []
    while pos + 12 <= n:
        length, ctype = struct.unpack_from(">I4s", buf, pos)
        end = pos + 12 + length
        if end > n or not ctype.isalpha():
            break
        if ctype == b"IDAT":
            idat.append((pos, length))
        elif idat:
            break
        pos = end
    if not idat:
        return None
    start = idat[0][0]
    stop = idat[-1][0] + 12 + idat[-1][1]
    skip = ("skip", stop)
    for chunk, length in idat:
        if zlib.crc32(buf[chunk + 4:chunk + 8 + length]) != struct.unpack_from(">I", buf, chunk + 8 + length)[0]:
            return skip
    payload = b"".join(buf[chunk + 8:chunk + 8 + length] for chunk, length in idat)
    if not attempt or len(payload) < MIN_COMP or payload[0] & 15 != 8 or payload[0] >> 4 == 0:
        return skip
    res = _inflate(payload, 0, 15, waste)
    if res is None or res[0] is None or res[1] != len(payload) or len(res[0]) < MIN_BLOB:
        return skip
    params = ctx.finder.find(res[0], payload, (payload[0] >> 4) + 8)
    if params is None:
        return skip
    sizes = []
    for _, length in idat:
        if sizes and sizes[-1][0] == length:
            sizes[-1][1] += 1
        else:
            sizes.append([length, 1])
    return (start, stop, K_PNG, params, sizes, res[0])

def scan(buf, ctx):
    hits = []
    pos, n = 0, len(buf)
    good = bad = 0
    waste = [0]
    waste_limit = 4 * n + (64 << 20)
    while pos < n and waste[0] <= waste_limit:
        match = SCAN_RE.search(buf, pos)
        if match is None:
            break
        at = match.start()
        first = buf[at]
        attempt = good > 0 or bad < 32
        if first == 0x50:
            res = _zip_entry(buf, at, ctx, attempt, waste)
        elif first == 0x1F:
            res = _gzip_member(buf, at, ctx, attempt, waste)
        elif first == 0x89:
            res = _png(buf, at, ctx, attempt, waste)
        else:
            res = _zlib_stream(buf, at, ctx, attempt, waste)
        if res is None:
            pos = at + 1
        elif len(res) == 2:
            bad += 1
            pos = res[1]
        else:
            good += 1
            hits.append(res)
            pos = res[1]
    return hits

TEXT_BYTES = bytes(range(32, 127)) + b"\t\r\n" + bytes(range(128, 256))
_LZ_FAST = {"id": lzma.FILTER_LZMA2, "preset": 0}
STORE_ABOVE = [0.995]

def classify(data):
    n = len(data)
    if n > 32768:
        step = (n - 4096) // 7
        samples = [data[i * step:i * step + 4096] for i in range(8)]
    else:
        samples = [data]
    sample = b"".join(samples)
    size = len(sample)
    if b"\0" not in sample and len(sample.translate(None, TEXT_BYTES)) * 20 < size:
        return "text"
    if n < 4096:
        return "bin"
    quick = len(zlib.compress(sample, 1))
    base = None
    if quick > 0.9 * size:
        base = len(lzma.compress(sample, format=lzma.FORMAT_RAW, filters=[_LZ_FAST]))
    packed = base is not None and base > STORE_ABOVE[0] * size
    if data[:2] == b"MZ" or data[:4] == b"\x7fELF":
        return "store" if packed else "x86"
    if n >= 65536 and quick > 0.4 * size:
        if base is None:
            base = len(lzma.compress(sample, format=lzma.FORMAT_RAW, filters=[_LZ_FAST]))
        best, dist = base, 0
        for d in (1, 2, 3, 4):
            chain = [{"id": lzma.FILTER_DELTA, "dist": d}, _LZ_FAST]
            cost = len(lzma.compress(sample, format=lzma.FORMAT_RAW, filters=chain))
            if cost < best:
                best, dist = cost, d
        if dist and best < 0.94 * base:
            return "d%d" % dist
    return "store" if packed else "bin"

def filter_chain(codec, dict_size, preset=None):
    lz = {"id": lzma.FILTER_LZMA2, "dict_size": dict_size}
    if preset is not None:
        lz["preset"] = preset
        if codec == "text":
            lz["pb"] = 0
    if codec == "x86":
        return [{"id": lzma.FILTER_X86}, lz]
    if codec[0] == "d":
        return [{"id": lzma.FILTER_DELTA, "dist": int(codec[1:])}, lz]
    return [lz]

def compress_block(codec, data, level):
    if codec != "store":
        dict_size = 1 << 16
        while dict_size < len(data) and dict_size < level["dict_size"]:
            dict_size <<= 1
        comp = lzma.compress(data, format=lzma.FORMAT_RAW, filters=filter_chain(codec, dict_size, level["preset"]))
        if len(comp) < len(data):
            return codec, dict_size, comp
    return "store", 0, bytes(data)

class Context:
    def __init__(self, known):
        self.finder = DeflateFinder()
        self.known = known

def unwrap(data, ctx, depth=0, h=None):
    if h is None:
        h = digest(data)
    if h in ctx.known:
        return ("L", h, data, None)
    if depth < MAX_DEPTH and len(data) >= MIN_BLOB:
        pieces, pos = [], 0
        for start, end, kind, params, extra, raw in scan(data, ctx):
            child = unwrap(raw, ctx, depth + 1)
            doubtful = child[0] == "L" and child[3] == "store" and len(raw) >= end - start
            pieces.append(data[pos:start])
            pieces.append((kind, params, extra, child, data[start:end] if doubtful else None))
            pos = end
        if pieces:
            pieces.append(data[pos:])
            return ("C", h, len(data), pieces)
    return ("L", h, data, classify(data) if data else None)

class Packer:
    def __init__(self, out_path, level, threads):
        self.level = level
        STORE_ABOVE[0] = level["store_above"]
        self.out = open(out_path, "wb")
        self.out.write(MAGIC)
        per_block = int(level["dict_size"] * 11.5) + 2 * level["block"]
        block_workers = max(1, min(threads, int(total_ram() * 0.5) // per_block))
        self.pool = ThreadPoolExecutor(threads)
        self.block_pool = ThreadPoolExecutor(block_workers)
        self.max_pending = block_workers + 2
        self.pending = deque()
        self.blocks = []
        self.current = {}
        self.nodes = []
        self.by_hash = {}
        self.chunks = {}
        self.params = {}
        self.ctx = Context(self.by_hash)
        self.unwrapped = self.unwrapped_bytes = self.dedup_bytes = 0

    def _flush(self, cls):
        bid, buf = self.current.pop(cls)
        self.pending.append((bid, self.block_pool.submit(compress_block, cls, buf, self.level), len(buf)))
        while len(self.pending) > self.max_pending:
            self._write_oldest()

    def _write_oldest(self):
        bid, future, raw_len = self.pending.popleft()
        codec, dict_size, comp = future.result()
        self.blocks[bid] = [self.out.tell(), len(comp), raw_len, codec, dict_size]
        self.out.write(comp)

    @staticmethod
    def _extend(node, bid, offset, length):
        if node and node[-3] == bid and node[-2] + node[-1] == offset:
            node[-1] += length
        else:
            node.extend((bid, offset, length))

    def _place(self, node, data, cls, fresh=(), base=0):
        cls = cls or classify(data)
        cur = self.current.get(cls)
        if cur is None:
            cur = self.current[cls] = (len(self.blocks), bytearray())
            self.blocks.append(None)
        bid, buf = cur
        offset = len(buf)
        buf.extend(data)
        for chunk_hash, start in fresh:
            self.chunks[chunk_hash] = (bid, offset + start - base)
        self._extend(node, bid, offset, len(data))
        if len(buf) >= self.level["block"]:
            self._flush(cls)

    def add_leaf(self, data, cls, h):
        nid = self.by_hash.get(h)
        if nid is not None:
            self.dedup_bytes += len(data)
            return nid
        node = []
        if not data:
            node = [-1, 0, 0]
        elif len(data) < CHUNKED_FROM:
            self._place(node, data, cls)
        else:
            hint = cls if len(data) <= SEGMENT else None
            view = memoryview(data)
            fresh, seg, pos = [], 0, 0
            for cut in cut_points(data):
                chunk_hash = digest(view[pos:cut])
                ref = self.chunks.get(chunk_hash)
                if ref is None:
                    fresh.append((chunk_hash, pos))
                    if cut - seg >= SEGMENT:
                        self._place(node, data[seg:cut], hint, fresh, seg)
                        fresh, seg = [], cut
                else:
                    if pos > seg:
                        self._place(node, data[seg:pos], hint, fresh, seg)
                        fresh = []
                    self._extend(node, ref[0], ref[1], cut - pos)
                    self.dedup_bytes += cut - pos
                    seg = cut
                pos = cut
            if pos > seg:
                self._place(node, data[seg:pos], hint, fresh, seg)
        nid = len(self.nodes)
        self.nodes.append(node)
        self.by_hash[h] = nid
        return nid

    def _mostly_known(self, leaf):
        data = leaf[2]
        if leaf[1] in self.by_hash:
            return True
        if len(data) < CHUNKED_FROM:
            return False
        view = memoryview(data)
        known = pos = 0
        for cut in cut_points(data):
            if digest(view[pos:cut]) in self.chunks:
                known += cut - pos
            pos = cut
        return known * 2 >= len(data)

    def intern(self, tree):
        h = tree[1]
        nid = self.by_hash.get(h)
        if nid is not None:
            self.dedup_bytes += tree[2] if tree[0] == "C" else len(tree[2])
            return nid
        if tree[0] == "L":
            return self.add_leaf(tree[2], tree[3], h)
        size, pieces = tree[2], tree[3]
        skeleton, entries, gap = [], [], 0
        for piece in pieces:
            if isinstance(piece, bytes):
                skeleton.append(piece)
                gap += len(piece)
                continue
            kind, params, extra, child, original = piece
            if original is not None and not self._mostly_known(child):
                skeleton.append(original)
                gap += len(original)
                continue
            entry = [gap, kind, self.params.setdefault(params, len(self.params)), self.intern(child)]
            if extra:
                entry.append(extra)
            entries.append(entry)
            gap = 0
            self.unwrapped += 1
            self.unwrapped_bytes += child[2] if child[0] == "C" else len(child[2])
        skeleton = b"".join(skeleton)
        if not entries:
            return self.add_leaf(skeleton, None, h)
        skeleton_cls = "skel" if len(skeleton) <= SEGMENT else None
        node = [self.add_leaf(skeleton, skeleton_cls, digest(skeleton)), size, entries]
        nid = len(self.nodes)
        self.nodes.append(node)
        self.by_hash[h] = nid
        return nid

    def run(self, files, dirs, progress):
        entries, window, window_bytes = [], deque(), 0
        first_by_hash = {}
        skipped = []
        window_len = 4 * self.pool._max_workers

        def resolve():
            nonlocal window_bytes
            parts, future, size = window.popleft()
            window_bytes -= size
            parts.append(self.intern(future.result()))

        for arc, full in files:
            path = long_path(full)
            try:
                stat = os.stat(path)
                if stat.st_size > BIG_FILE:
                    while window:
                        resolve()
                    hasher = hashlib.blake2b(digest_size=16)
                    parts, size = [], 0
                    with open(path, "rb") as f:
                        while True:
                            piece = f.read(PIECE)
                            if not piece:
                                break
                            hasher.update(piece)
                            size += len(piece)
                            parts.append(self.add_leaf(piece, None, digest(piece)))
                            progress(len(piece))
                    entries.append([arc, size, stat.st_mtime_ns, hasher.hexdigest(), parts])
                    continue
                with open(path, "rb") as f:
                    data = f.read()
            except OSError as exc:
                skipped.append((arc, str(exc)))
                continue
            h = digest(data)
            parts = first_by_hash.get(h)
            if parts is None:
                parts = first_by_hash[h] = []
                if data:
                    window.append((parts, self.pool.submit(unwrap, data, self.ctx, 0, h), len(data)))
                    window_bytes += len(data)
            else:
                self.dedup_bytes += len(data)
            entries.append([arc, len(data), stat.st_mtime_ns, h.hex(), parts])
            progress(len(data))
            while window and (window_bytes > WINDOW_BYTES or len(window) > window_len):
                resolve()
        while window:
            resolve()
        for cls in list(self.current):
            self._flush(cls)
        while self.pending:
            self._write_oldest()

        index = {
            "version": 1,
            "zlib": zlib.ZLIB_RUNTIME_VERSION,
            "blocks": self.blocks,
            "nodes": self.nodes,
            "params": [list(p) for p in self.params],
            "files": entries,
            "dirs": dirs,
        }
        comp = lzma.compress(json.dumps(index, separators=(",", ":")).encode(), preset=9)
        offset = self.out.tell()
        self.out.write(comp)
        self.out.write(FOOTER.pack(offset, len(comp), END_MAGIC))
        self.out.close()
        return entries, skipped

    def close(self):
        self.pool.shutdown(wait=False, cancel_futures=True)
        self.block_pool.shutdown(wait=False, cancel_futures=True)
        if not self.out.closed:
            self.out.close()

def collect(inputs, out_path):
    files, dirs, roots = [], [], set()
    out_path = os.path.normcase(os.path.abspath(out_path))
    for inp in inputs:
        full = os.path.abspath(inp)
        base = os.path.basename(full.rstrip("\\/")) or "root"
        if base.lower() in roots:
            raise SystemExit("Zwei Eingaben heissen gleich: %s" % base)
        roots.add(base.lower())
        if os.path.isfile(full):
            files.append((base, full))
        elif os.path.isdir(full):
            dirs.append(base)
            for root, dir_names, file_names in os.walk(full):
                rel = os.path.relpath(root, full)
                prefix = base if rel == "." else base + "/" + rel.replace(os.sep, "/")
                dirs.extend(prefix + "/" + d for d in dir_names)
                for name in file_names:
                    path = os.path.join(root, name)
                    if os.path.islink(path) or os.path.normcase(path) in (out_path, out_path + ".part"):
                        continue
                    files.append((prefix + "/" + name, path))
        else:
            raise SystemExit("Nicht gefunden: %s" % inp)

    def order(item):
        name = item[0].lower()
        base = name.rsplit("/", 1)[-1]
        return (base.rsplit(".", 1)[-1] if "." in base else "", base, name)

    files.sort(key=order)
    return files, dirs

class Progress:
    def __init__(self, label, total, quiet):
        self.label, self.total, self.quiet = label, max(total, 1), quiet
        self.done = 0
        self.last = 0.0
        self.lock = threading.Lock()

    def __call__(self, amount):
        with self.lock:
            self.done += amount
            now = time.perf_counter()
            if self.quiet or now - self.last < 0.5:
                return
            self.last = now
            sys.stderr.write("\r%s %5.1f %%  (%.0f MB)   " % (
                self.label, 100.0 * self.done / self.total, self.done / 1e6))
            sys.stderr.flush()

    def finish(self):
        if not self.quiet:
            sys.stderr.write("\r" + " " * 60 + "\r")
            sys.stderr.flush()

def cmd_pack(args):
    out_path = args.output
    if not out_path:
        if len(args.inputs) != 1:
            raise SystemExit("Bei mehreren Eingaben bitte -o ARCHIV.qz angeben.")
        out_path = os.path.basename(os.path.abspath(args.inputs[0]).rstrip("\\/")) + ".qz"
    if os.path.exists(out_path) and not args.force:
        raise SystemExit("%s existiert schon (--force ueberschreibt)." % out_path)
    files, dirs = collect(args.inputs, out_path)
    total = 0
    for _, full in files:
        try:
            total += os.path.getsize(long_path(full))
        except OSError:
            pass
    start = time.perf_counter()
    part = out_path + ".part"
    packer = Packer(part, LEVELS[args.level], args.threads)
    progress = Progress("Packe", total, args.quiet)
    try:
        entries, skipped = packer.run(files, dirs, progress)
    except BaseException:
        packer.close()
        if os.path.exists(part):
            os.remove(part)
        raise
    packer.close()
    progress.finish()
    os.replace(part, out_path)
    size_in = sum(entry[1] for entry in entries)
    size_out = os.path.getsize(out_path)
    print("%d Dateien, %.1f MB -> %.1f MB (%.1f %%) in %.1f s" % (
        len(entries), size_in / 1e6, size_out / 1e6, 100.0 * size_out / max(size_in, 1),
        time.perf_counter() - start))
    print("ausgepackte Streams: %d (%.1f MB Rohdaten), doppelt gespart: %.1f MB" % (
        packer.unwrapped, packer.unwrapped_bytes / 1e6, packer.dedup_bytes / 1e6))
    for arc, reason in skipped:
        print("UEBERSPRUNGEN: %s (%s)" % (arc, reason))
    return 1 if skipped else 0

class Reader:
    def __init__(self, path):
        self.file = open(path, "rb")
        self.file.seek(0, os.SEEK_END)
        end = self.file.tell()
        if end < len(MAGIC) + FOOTER.size:
            raise SystemExit("Kein Quarz-Archiv: %s" % path)
        self.file.seek(end - FOOTER.size)
        offset, length, magic = FOOTER.unpack(self.file.read(FOOTER.size))
        self.file.seek(0)
        if magic != END_MAGIC or self.file.read(len(MAGIC)) != MAGIC or offset + length > end:
            raise SystemExit("Kein Quarz-Archiv oder beschaedigt: %s" % path)
        self.file.seek(offset)
        index = json.loads(lzma.decompress(self.file.read(length)))
        self.index = index
        self.blocks, self.nodes, self.files = index["blocks"], index["nodes"], index["files"]
        self.params = [tuple(p) for p in index["params"]]
        self.io_lock = threading.Lock()
        self.cache_lock = threading.Lock()
        self.block_locks = [threading.Lock() for _ in self.blocks]
        self.cache = OrderedDict()
        self.cache_bytes = 0
        self.cache_limit = min(total_ram() // 4, 6 << 30)

    def block(self, bid):
        with self.block_locks[bid]:
            with self.cache_lock:
                data = self.cache.get(bid)
                if data is not None:
                    self.cache.move_to_end(bid)
                    return data
            offset, length, raw_len, codec, dict_size = self.blocks[bid]
            with self.io_lock:
                self.file.seek(offset)
                data = self.file.read(length)
            if codec != "store":
                data = lzma.decompress(data, format=lzma.FORMAT_RAW, filters=filter_chain(codec, dict_size))
            if len(data) != raw_len:
                raise ValueError("Block %d ist beschaedigt" % bid)
            with self.cache_lock:
                self.cache[bid] = data
                self.cache_bytes += len(data)
                while self.cache_bytes > self.cache_limit and len(self.cache) > 1:
                    self.cache_bytes -= len(self.cache.popitem(last=False)[1])
            return data

    def build(self, nid):
        node = self.nodes[nid]
        if not isinstance(node[2], list):
            if node[0] < 0:
                return b""
            if len(node) == 3:
                return self.block(node[0])[node[1]:node[1] + node[2]]
            return b"".join(self.block(node[i])[node[i + 1]:node[i + 1] + node[i + 2]]
                            for i in range(0, len(node), 3))
        skeleton = self.build(node[0])
        out, pos = [], 0
        for entry in node[2]:
            gap, kind, pid, child = entry[:4]
            out.append(skeleton[pos:pos + gap])
            pos += gap
            comp = b"".join(deflate_pieces(self.build(child), *self.params[pid]))
            if kind == K_PNG:
                at = 0
                for size, count in entry[4]:
                    for _ in range(count):
                        chunk = b"IDAT" + comp[at:at + size]
                        at += size
                        out.append(struct.pack(">I", size) + chunk + struct.pack(">I", zlib.crc32(chunk)))
                if at != len(comp):
                    raise ValueError("PNG-Daten passen nicht")
            else:
                out.append(comp)
        out.append(skeleton[pos:])
        data = b"".join(out)
        if len(data) != node[1]:
            raise ValueError("Knoten %d hat die falsche Laenge" % nid)
        return data

def safe_target(dest, name):
    parts = name.split("/")
    if not name or any(p in ("", ".", "..") or ":" in p or "\\" in p for p in parts):
        raise ValueError("unsicherer Pfad im Archiv: %r" % name)
    return os.path.join(dest, *parts)

def extract(reader, dest, threads, quiet):
    if dest is not None:
        for name in reader.index["dirs"]:
            os.makedirs(long_path(safe_target(dest, name)), exist_ok=True)
    progress = Progress("Entpacke" if dest is not None else "Pruefe", sum(e[1] for e in reader.files), quiet)

    def one(entry):
        name, size, mtime, expected, parts = entry
        hasher = hashlib.blake2b(digest_size=16)
        try:
            if dest is None:
                for nid in parts:
                    hasher.update(reader.build(nid))
            else:
                target = long_path(safe_target(dest, name))
                os.makedirs(os.path.dirname(target), exist_ok=True)
                with open(target, "wb") as f:
                    for nid in parts:
                        data = reader.build(nid)
                        hasher.update(data)
                        f.write(data)
                os.utime(target, ns=(mtime, mtime))
        except Exception as exc:
            return "%s (%s)" % (name, exc)
        progress(size)
        return None if hasher.hexdigest() == expected else "%s (Pruefsumme falsch)" % name

    with ThreadPoolExecutor(threads) as pool:
        failed = [res for res in pool.map(one, reader.files) if res]
    progress.finish()
    return failed

def cmd_unpack(args, verify_only=False):
    reader = Reader(args.archive)
    dest = None
    if not verify_only:
        dest = args.output or os.path.splitext(os.path.basename(args.archive))[0]
        if os.path.isdir(dest) and os.listdir(dest) and not args.force:
            raise SystemExit("%s ist nicht leer (--force schreibt trotzdem hinein)." % dest)
        os.makedirs(dest, exist_ok=True)
    if reader.index["zlib"] != zlib.ZLIB_RUNTIME_VERSION:
        print("Hinweis: gepackt mit zlib %s, hier laeuft %s." % (reader.index["zlib"], zlib.ZLIB_RUNTIME_VERSION))
    start = time.perf_counter()
    failed = extract(reader, dest, args.threads, args.quiet)
    size = sum(entry[1] for entry in reader.files)
    print("%d Dateien, %.1f MB %s in %.1f s" % (
        len(reader.files), size / 1e6, "geprueft" if verify_only else "entpackt", time.perf_counter() - start))
    for item in failed:
        print("FEHLER: " + item)
    return 2 if failed else 0

def cmd_list(args):
    reader = Reader(args.archive)
    for name, size, *_ in sorted(reader.files):
        print("%12d  %s" % (size, name))
    size = sum(entry[1] for entry in reader.files)
    packed = os.path.getsize(args.archive)
    print("%d Dateien, %.1f MB, im Archiv %.1f MB (%.1f %%)" % (
        len(reader.files), size / 1e6, packed / 1e6, 100.0 * packed / max(size, 1)))
    return 0

def main():
    parser = argparse.ArgumentParser(prog="qz", description="Quarz-Archivierer (Prototyp)")
    parser.add_argument("--threads", type=int, default=os.cpu_count() or 4)
    parser.add_argument("--quiet", action="store_true")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("pack", help="Dateien und Ordner packen")
    p.add_argument("inputs", nargs="+")
    p.add_argument("-o", "--output")
    p.add_argument("--level", choices=sorted(LEVELS), default="normal")
    p.add_argument("--force", action="store_true")
    p = sub.add_parser("unpack", help="Archiv entpacken")
    p.add_argument("archive")
    p.add_argument("-o", "--output")
    p.add_argument("--force", action="store_true")
    p = sub.add_parser("test", help="Archiv vollstaendig pruefen, ohne etwas zu schreiben")
    p.add_argument("archive")
    p = sub.add_parser("list", help="Inhalt anzeigen")
    p.add_argument("archive")
    args = parser.parse_args()
    if args.command == "pack":
        return cmd_pack(args)
    if args.command == "unpack":
        return cmd_unpack(args)
    if args.command == "test":
        return cmd_unpack(args, verify_only=True)
    return cmd_list(args)

if __name__ == "__main__":
    sys.exit(main())
