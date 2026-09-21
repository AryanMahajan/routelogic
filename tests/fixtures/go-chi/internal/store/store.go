package store

// DB is the notes store.
type DB struct{}

// Open opens the store.
func Open(path string) *DB {
	return &DB{}
}
