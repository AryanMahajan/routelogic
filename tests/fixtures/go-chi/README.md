# Go (chi + net/http) fixture

A notes API in the shape chi projects take: a `Server` type that owns a router and serves it
from `ServeHTTP`, resources that each return a router from `Routes()`, `Route` closures for
nested paths, and a `Mount` per resource. Next to it, a Go 1.22 `http.ServeMux` with method
patterns, mounted under `/v2` through `http.StripPrefix`.

Deliberately included:

- `tags.Routes()` is mounted twice — at `/tags` and at `/labels`.
- `internal/legacy` builds a `ServeMux` nothing serves.
- the `admin` handler lives under a prefix read from the environment.
- `search` registers its handler through a table, which static analysis is expected to
  miss.
