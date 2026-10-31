The test suite never touches the network: every upstream
interaction in it runs against a local replay server, so it is
green without credentials and without quota.

Capability claims are not all of one kind, and the difference is
recorded rather than smoothed over. Most have since been settled
against a live subscription — `docs/roadmap.md` §L lists each
question, its answer, and the four things the offline work believed
that a live backend falsified. A few remain derived rather than
confirmed, and §L says which. `proxenos doctor` makes the same
distinction in its own output: a replayed run says so, and a probe
that could not run is reported as skipped rather than passed.
