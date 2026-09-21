# Go (Gin) fixture

A small shop API in the layout Gin projects usually have: `cmd/server/main.go` builds the
engine and the groups, each `internal/<area>` package registers its own routes on the
group it is handed, and handlers live in a file of their own next to the routes.

Deliberately included:

- `orders.Register` is mounted twice — under `/api/v1` and under `/legacy`.
- `legacy.Register` is never called, so its route is unreachable.
- `admin` groups its routes under a prefix read from configuration.
- `reports` registers one route from a method held in a variable, which static analysis
  is expected to miss, and one in a loop, whose path it reports as unresolved.
