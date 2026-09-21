package users

import (
	"github.com/gin-gonic/gin"

	"example.com/shop/internal/middleware"
)

// Register mounts the users API on the group it is given.
func Register(rg *gin.RouterGroup) {
	h := &Handler{}

	rg.GET("", h.List)
	rg.GET("/:id", h.Get)
	rg.POST("", middleware.AuthRequired(), h.Create)
	rg.PUT("/:id", middleware.AuthRequired(), h.Update)
	rg.DELETE("/:id", middleware.AuthRequired(), h.Delete)
	rg.POST("/:id/avatar", middleware.AuthRequired(), h.Avatar)
}
