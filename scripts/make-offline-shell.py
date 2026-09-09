#!/usr/bin/env python3
"""Give a built app its own copy of itself, so it opens with the internet down.

Reads a built app's directory, writes `sw.js` into it carrying the list of every
file that build is made of. The list comes from the build rather than from
anybody's memory: a hand-written list is a list missing the file the bundler
renamed this time, and the shop finds out when the tablet is switched on during
an outage and shows a browser error.

The rules the worker follows are `apps/shared/offline_shell.js`, pasted in with
their `export` keywords removed. Pasted rather than imported because a worker
importing a sibling has to be a module worker served from a path that resolves,
and the alternative is a second bundler entry point. One source of truth either
way, and this one is twelve lines.

    make-offline-shell.py <built-app-dir> <base-path>
"""

import hashlib
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    built, base = Path(sys.argv[1]), sys.argv[2]

    # Every file the build produced, as the browser will ask for it. Maps are
    # left out: they are for whoever is debugging and are often larger than the
    # code they describe.
    files = sorted(
        f"{base}{p.relative_to(built).as_posix()}"
        for p in built.rglob("*")
        if p.is_file() and not p.name.endswith(".map") and p.name != "sw.js"
    )
    # The address somebody actually opens. A copy holding index.html and asked
    # for / answers nothing.
    files = sorted({base, *files})

    # Named for what is in it, so a build that changed nothing keeps its copy
    # and a build that changed anything gets a new one. A version number would
    # need somebody to remember to change it.
    digest = hashlib.sha256()
    for path in sorted(p for p in built.rglob("*") if p.is_file() and p.name != "sw.js"):
        digest.update(path.relative_to(built).as_posix().encode())
        digest.update(path.read_bytes())
    build_id = digest.hexdigest()[:12]

    # A copy without the core is a copy that boots to a blank screen, and a copy
    # without the page is a copy that answers nothing. Both are silent: the
    # build succeeds, the app works everywhere it can reach its server, and the
    # shop finds out on the morning the line is down. Checked here because here
    # is where the list is made.
    if not any(one.endswith(".wasm") for one in files):
        print("no wasm in the copy: the app would boot to a blank screen", file=sys.stderr)
        return 1
    if not any(one.endswith("index.html") for one in files):
        print("no page in the copy: the app would not open at all", file=sys.stderr)
        return 1

    # Every file the page names has to be in the copy. The bundler renames its
    # assets on every build, and a copy that holds a page pointing at a script
    # it does not have is a device that boots to a blank screen the first time
    # it is opened without a line. Nothing else notices: the build succeeds, the
    # app works wherever it can reach its server, and the shop finds out on the
    # morning it cannot.
    page = (built / "index.html").read_text()
    for named in re.findall(r'(?:src|href)="([^"]+)"', page):
        if named.startswith(("http://", "https://", "//", "data:")):
            continue
        wanted = named if named.startswith("/") else f"{base}{named.lstrip('./')}"
        if wanted not in files:
            print(
                f"the page names {wanted} and the copy would not hold it",
                file=sys.stderr,
            )
            return 1

    rules = (HERE / "apps/shared/offline_shell.js").read_text()
    rules = re.sub(r"^export ", "", rules, flags=re.MULTILINE)

    worker = (HERE / "apps/shared/sw.js").read_text()
    worker = worker.replace(
        "// __OPENPOS_BUILD__\nconst BUILD = 'unbuilt';\nconst BASE = '/';\nconst FILES = [];",
        "// Written by scripts/make-offline-shell.py from the build itself.\n"
        f"const BUILD = '{build_id}';\n"
        f"const BASE = '{base}';\n"
        "const FILES = [\n"
        + "".join(f"  '{one}',\n" for one in files)
        + "];",
    )
    worker = re.sub(
        r"// __OPENPOS_RULES__\n(//[^\n]*\n)+",
        "// The rules, from apps/shared/offline_shell.js, which has the tests.\n" + rules,
        worker,
        count=1,
    )
    if "__OPENPOS_BUILD__" in worker or "__OPENPOS_RULES__" in worker:
        print("the worker template did not take its build or its rules", file=sys.stderr)
        return 1

    (built / "sw.js").write_text(worker)
    print(f"  {base} keeps a copy of itself: {len(files)} files, build {build_id}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
