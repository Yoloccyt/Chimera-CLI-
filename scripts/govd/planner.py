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