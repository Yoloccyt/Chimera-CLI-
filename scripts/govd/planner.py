"""Selection, dependency ordering and sharding for govd (ADR-228 §3.3)."""
from __future__ import annotations

from .model import Profile
from .policy import select


class PlanError(ValueError):
    """The plan cannot be built; the caller must exit 2."""


def toposort(tasks):
    """Stable DFS topo-sort; only declared `needs` edges constrain order."""
    by_id = {t.id: t for t in tasks}
    temp, done, order = set(), set(), []

    def visit(tid, stack):
        if tid in done:
            return
        if tid in temp:
            raise PlanError("needs cycle: %s" % " -> ".join(stack + [tid]))
        temp.add(tid)
        for need in by_id[tid].needs:
            if need in by_id:
                visit(need, stack + [tid])
        temp.remove(tid)
        done.add(tid)
        order.append(by_id[tid])

    for task in tasks:
        visit(task.id, [])
    return order

def plan(tasks, profile: Profile, allow_mutating: bool):
    """Return (ordered, skipped). `skipped` still gets a journal verdict."""
    selected, skipped = select(tasks, profile, allow_mutating)
    return toposort(selected), skipped


def shards(ordered, jobs: int):
    """Round-robin shards keep deterministic output order when merged."""
    if jobs <= 1:
        return [list(ordered)]
    return [list(ordered[i::jobs]) for i in range(jobs) if ordered[i::jobs]]


def parse_shard(spec):
    """`k/n` -> (k, n); empty spec means "everything".

    Malformed or out-of-range specs raise PlanError so the caller exits 2: a
    typo like `--shard 5/4` must not silently run a different subset (that is
    the same silent-degradation family as F24's unknown-flag bug).
    """
    if not spec:
        return None
    try:
        index, total = (int(part) for part in spec.split("/"))
    except ValueError:
        raise PlanError("shard must be k/n with integers, got %r" % spec)
    if total < 1 or index < 1 or index > total:
        raise PlanError("shard %r out of range (need 1 <= k <= n)" % spec)
    return index, total


def apply_shard(ordered, spec):
    """Return (selected, note). Refuses to split a plan that has `needs` edges.

    WHY refuse instead of reorder: a declared dependency can land in another
    shard, and then the shard either skips its precondition or the ordering
    guarantee silently stops meaning anything. Today no gate declares `needs`,
    so the refusal costs nothing -- but it costs everything the day someone
    adds one and CI starts sharding by default.
    """
    parsed = parse_shard(spec)
    if parsed is None:
        return ordered, ""
    if any(task.needs for task in ordered):
        raise PlanError("--shard cannot split a plan that declares needs edges")
    index, total = parsed
    parts = shards(ordered, total)
    if index > len(parts):          # fewer gates than shards: an empty shard is
        return [], spec             # legitimate, but it must be visible as such
    return parts[index - 1], spec