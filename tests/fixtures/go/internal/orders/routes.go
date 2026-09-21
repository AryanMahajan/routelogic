package orders

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

const prefix = "/orders"

// Register mounts the orders API, under its own prefix, on the group it is given.
func Register(rg *gin.RouterGroup) {
	g := rg.Group(prefix)
	g.GET("", list)
	g.GET("/:orderId", get)
	g.GET("/:orderId/items", items)
	g.POST("", create)
}

type Order struct {
	ID     string `json:"id"`
	Status string `json:"status"`
}

type CreateOrder struct {
	Items []Item `json:"items" binding:"required"`
	Note  string `json:"note"`
}

type Item struct {
	SKU      string `json:"sku"`
	Quantity int    `json:"quantity"`
}

func list(c *gin.Context) {
	status := c.Query("status")
	_ = status
	c.JSON(http.StatusOK, []Order{})
}

func get(c *gin.Context) {
	c.JSON(http.StatusOK, Order{ID: c.Param("orderId")})
}

func items(c *gin.Context) {
	c.JSON(http.StatusOK, []Item{})
}

func create(c *gin.Context) {
	var req CreateOrder
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusCreated, Order{})
}
