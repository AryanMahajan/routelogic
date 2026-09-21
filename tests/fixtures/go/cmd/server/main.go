package main

import (
	"log"
	"net/http"

	"github.com/gin-gonic/gin"

	"example.com/shop/internal/admin"
	"example.com/shop/internal/middleware"
	"example.com/shop/internal/orders"
	"example.com/shop/internal/reports"
	"example.com/shop/internal/users"
)

const apiPrefix = "/api"

func main() {
	r := gin.Default()
	r.Use(gin.Logger(), gin.Recovery())

	r.GET("/health", func(c *gin.Context) {
		c.JSON(http.StatusOK, gin.H{"ok": true})
	})

	v1 := r.Group(apiPrefix + "/v1")
	users.Register(v1.Group("/users"))
	orders.Register(v1)

	// The old clients still call the orders API under /legacy.
	orders.Register(r.Group("/legacy"))

	protected := v1.Group("/admin", middleware.AuthRequired())
	admin.Register(protected, admin.Config{Prefix: "/internal"})
	reports.Register(protected)

	r.NoRoute(func(c *gin.Context) {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
	})

	log.Fatal(r.Run(":8080"))
}
