#!/usr/bin/env python3
"""Folder search on this machine for the Add project flow, one JSON request per run.

The ranking follows Paseo's directory suggestions: a fragment walks the search roots, and a typed
path anchors the walk under its parent. It reads directory entries and asks whether `.git` exists.
It never opens a file. The service supplies the home, the roots and every limit; a client only
ever sends the text the owner typed.
"""
from collections import deque
import json
import os
import re
import sys
import threading
import time

DEFAULT_LIMIT = 30
MAX_LIMIT = 100
MAX_DEPTH = 12
MAX_FOLDERS = 20_000
MAX_ENTRIES = 200_000
MAX_DIRECTORY_ENTRIES = 50_000
DEADLINE_MS = 2_000
CONFIDENT_AFTER = 5_000
# The answer a stuck read holds back is sent this long after the deadline.
WATCHDOG_GRACE_MS = 300
IGNORED = {"node_modules", "venv", "env", "virtualenv", "dist", "build", "target", "out", "coverage", "vendor", "__pycache__", ".git"}
PRIVATE_NAMES = {".cortex", ".cortex-dev"}
# Kernel trees are not folders anyone keeps a project in. Naming one still lists it.
NEVER_ENTERED = {"/proc", "/sys", "/dev"}
TIER_NONE = 5
FAR = 1 << 62
WORD = re.compile(r"[a-z0-9]")


def boundary(ch):
    return ch is None or not WORD.match(ch)


def substring_score(query, text):
    best = None
    position = text.find(query)
    while position >= 0:
        before = text[position - 1] if position else None
        end = position + len(query)
        after = text[end] if end < len(text) else None
        starts = position == 0 or boundary(before)
        if starts and boundary(after):
            tier = 1
        elif position == 0:
            tier = 2
        elif starts:
            tier = 3
        else:
            tier = 4
        if best is None or (tier, position) < best[:2]:
            best = (tier, position, 0)
        position = text.find(query, position + 1)
    return best


def subsequence_score(query, text):
    index = 0
    first = last = -1
    for position, ch in enumerate(text):
        if index == len(query):
            break
        if ch.isspace():
            index, first, last = 0, -1, -1
            continue
        if ch != query[index]:
            continue
        if first < 0:
            first = position
        last = position
        index += 1
    if index != len(query) or first < 0:
        return None
    return (5, first, last - first + 1)


def text_score(query, text):
    if text == query:
        return (0, 0, 0)
    return substring_score(query, text) or subsequence_score(query, text)


def compact(value):
    kept = [(ch, i) for i, ch in enumerate(value) if WORD.match(ch)]
    return "".join(ch for ch, _ in kept), [i for _, i in kept]


def path_score(query, path):
    """A query against a whole shown path, separators included."""
    query, path = query.lower(), path.lower()
    direct = text_score(query, path)
    if direct:
        return direct
    small_query, _ = compact(query)
    small_path, offsets = compact(path)
    if not small_query or not small_path:
        return None
    score = text_score(small_query, small_path)
    if not score:
        return None
    return (score[0], offsets[score[1]] if score[1] < len(offsets) else score[1], score[2])


def fuzzy_score(query, candidate):
    index, first, previous, gaps = 0, -1, -1, 0
    for position, ch in enumerate(candidate):
        if index == len(query):
            break
        if ch != query[index]:
            continue
        if first < 0:
            first = position
        if previous >= 0:
            gaps += position - previous - 1
        previous = position
        index += 1
    return first + gaps if index == len(query) and first >= 0 else None


def rank_path(query, relative):
    score = path_score(query, relative)
    if not score:
        return (TIER_NONE, FAR, FAR, FAR)
    return (min(score[0], 4), FAR, score[1], score[2])


def rank_text(query, relative):
    lowered = relative.lower()
    segments = lowered.split("/")
    offset = lowered.find(query)
    fuzzy = fuzzy_score(query, segments[-1])
    at = offset if offset >= 0 else FAR
    spread = fuzzy if fuzzy is not None else FAR
    for tier, matches in ((0, lambda s: s == query), (1, lambda s: s.startswith(query)), (2, lambda s: query in s)):
        for index, segment in enumerate(segments):
            if matches(segment):
                return (tier, index, at, spread)
    if offset >= 0:
        return (3, FAR, offset, spread)
    if fuzzy is not None:
        return (4, FAR, at, fuzzy)
    return rank_path(query, relative)


def inside(root, path):
    return path == root or root == "/" or path.startswith(root.rstrip("/") + "/")


class Search:
    def __init__(self, request):
        self.query = str(request.get("query", "")).strip()
        self.home = os.path.normpath(str(request.get("home") or "/"))
        self.allowed = os.path.normpath(str(request.get("allowedRoot") or "/"))
        self.roots = [os.path.normpath(str(root)) for root in request.get("roots") or [self.home]]
        self.excluded = [os.path.normpath(str(path)) for path in request.get("excluded") or []]
        self.limit = max(1, min(MAX_LIMIT, int(request.get("limit") or DEFAULT_LIMIT)))
        self.max_depth = int(request.get("maxDepth") or MAX_DEPTH)
        self.max_folders = int(request.get("maxFolders") or MAX_FOLDERS)
        self.max_entries = int(request.get("maxEntries") or MAX_ENTRIES)
        self.max_directory = int(request.get("maxDirectoryEntries") or MAX_DIRECTORY_ENTRIES)
        self.confident_after = int(request.get("confidentAfter") or CONFIDENT_AFTER)
        self.deadline = time.monotonic() + int(request.get("deadlineMs") or DEADLINE_MS) / 1000
        self.ranked = {}
        self.visited = set()
        self.folders = 0
        self.entries = 0
        self.stopped = None
        self.capped = False
        self.exact = None
        self.needle = ""
        self.as_path = False
        self.show_hidden = False

    # --- what the typed text names ---

    def typed_path(self):
        if self.query == "~" or self.query.startswith("~/"):
            return os.path.join(self.home, self.query[2:])
        if self.query == "." or self.query.startswith("./"):
            return os.path.join(self.home, self.query[2:])
        return self.query if self.query.startswith("/") else None

    def refused(self, path):
        if not inside(self.allowed, path) or PRIVATE_NAMES.intersection(path.split("/")):
            return True
        return any(inside(private, path) for private in self.excluded)

    def real(self, path):
        try:
            return os.path.realpath(path)
        except OSError:
            return None

    def is_directory(self, path):
        try:
            return os.path.isdir(path)
        except OSError:
            return False

    # --- walking ---

    def expired(self):
        if time.monotonic() >= self.deadline:
            self.stopped = "deadline"
            return True
        return False

    def children(self, real, visible, hidden=False):
        """Each folder directly inside one, as (name, shown path, real path). An unreadable one has none."""
        found = []
        seen = 0
        try:
            with os.scandir(real) as listing:
                for entry in listing:
                    seen += 1
                    self.entries += 1
                    if self.entries >= self.max_entries:
                        self.stopped = self.stopped or "limit"
                        break
                    if seen >= self.max_directory:
                        # One enormous folder ends its own listing, never the search.
                        self.capped = True
                        break
                    if seen % 256 == 0 and self.expired():
                        break
                    child = self.folder(entry, real, visible, hidden)
                    if child:
                        found.append(child)
        except OSError:
            return []
        found.sort()
        return found

    def folder(self, entry, real, visible, hidden):
        name = entry.name
        try:
            name.encode("utf-8")
        except UnicodeEncodeError:
            return None
        if name in PRIVATE_NAMES or name in IGNORED or (name.startswith(".") and not hidden):
            return None
        try:
            link = entry.is_symlink()
            if not (entry.is_dir() if link else entry.is_dir(follow_symlinks=False)):
                return None
        except OSError:
            return None
        target = self.real(entry.path) if link else os.path.join(real, name)
        if target is None or self.refused(target):
            return None
        return (name, os.path.join(visible, name), target, link)

    def consider(self, root, shown, depth):
        relative = os.path.relpath(shown, root)
        if self.needle:
            rank = (rank_path if self.as_path else rank_text)(self.needle, relative)
            if rank[0] == TIER_NONE:
                return
        else:
            rank = (0, FAR, FAR, FAR)
        key = (*rank, depth, shown)
        kept = self.ranked.get(shown)
        if kept is None or key < kept:
            self.ranked[shown] = key

    def confident(self):
        most = 2 * len(self.needle)
        return any(k[0] < 4 or (k[0] == 4 and k[3] <= most) for k in self.ranked.values())

    def walk(self, roots, depth_limit=None):
        """Breadth first, so every branch gets its turn before a deep one spends the budget."""
        depth_limit = self.max_depth if depth_limit is None else depth_limit
        pending = deque()
        for visible, real in roots:
            if real not in self.visited:
                self.visited.add(real)
                pending.append((visible, real, visible, 1))
        while pending:
            if self.stopped or self.expired():
                break
            visible, real, root, depth = pending.popleft()
            for name, shown, target, link in self.children(real, visible, self.show_hidden and depth == 1):
                self.folders += 1
                self.consider(root, shown, depth)
                if self.folders >= self.max_folders:
                    self.stopped = "limit"
                    break
                if self.folders >= self.confident_after and self.needle and self.confident():
                    self.stopped = "confident"
                    break
                # A link can reach anywhere. It is offered, and walked only while it stays in the tree.
                outside = link and not any(inside(r, target) for _, r in roots)
                if depth < depth_limit and target not in self.visited and target not in NEVER_ENTERED and not outside:
                    self.visited.add(target)
                    pending.append((shown, target, root, depth + 1))
            if self.stopped:
                break

    def run(self):
        typed = self.typed_path()
        if not self.query:
            return
        if typed is None:
            self.needle = self.query.lower()
            self.walk(self.search_roots())
            return
        browse = typed.endswith("/") or self.query in ("~", ".")
        path = os.path.normpath(typed)
        if self.refused(self.real(path) or path):
            return
        self.as_path = True
        if self.is_directory(path):
            self.exact = path
        if browse:
            real = self.real(path)
            if real and self.is_directory(real):
                self.walk([(path, real)], 1)
            return
        parent, term = os.path.split(path)
        real = self.real(parent)
        if real and self.is_directory(real):
            self.needle = term.lower()
            self.show_hidden = term.startswith(".")
            self.walk([(parent, real)])
        elif inside(self.home, path):
            # A mistyped parent still gets a chance against everything under the home.
            self.needle = os.path.relpath(path, self.home).lower()
            self.walk(self.search_roots())

    def search_roots(self):
        roots = []
        for root in self.roots:
            real = self.real(root)
            if real and self.is_directory(real) and not self.refused(real):
                roots.append((root, real))
        return roots

    # --- the answer ---

    def snapshot(self):
        ranked = sorted(list(self.ranked.values()))[: self.limit]
        paths = [key[-1] for key in ranked]
        if self.exact:
            paths = [self.exact, *[p for p in paths if p != self.exact]][: self.limit]
        return paths

    def reply(self, paths, repository):
        stopped = self.stopped or ("limit" if self.capped else None)
        rows = [{"path": path, "git": repository(path)} for path in paths]
        return {"entries": rows, "complete": stopped in (None, "confident"), "stopped": stopped, "scanned": self.folders}


def is_repository(path):
    try:
        return os.path.lexists(os.path.join(path, ".git"))
    except OSError:
        return False


def answer(value):
    sys.stdout.buffer.write((json.dumps(value, ensure_ascii=False) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()


def main():
    try:
        request = json.loads(sys.stdin.buffer.read().decode("utf-8"))
        search = Search(request)
    except (ValueError, TypeError, AttributeError):
        answer({"error": "invalid_directory_search"})
        return 0
    finished = threading.Event()

    def stuck():
        # A read on a hung mount never comes back. What was found so far is still the answer.
        if not finished.wait(max(0.0, search.deadline - time.monotonic()) + WATCHDOG_GRACE_MS / 1000):
            search.stopped = "deadline"
            answer(search.reply(search.snapshot(), lambda _path: False))
            os._exit(0)

    threading.Thread(target=stuck, daemon=True).start()
    search.run()
    reply = search.reply(search.snapshot(), is_repository)
    finished.set()
    answer(reply)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
