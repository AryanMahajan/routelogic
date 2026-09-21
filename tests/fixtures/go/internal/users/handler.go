package users

import (
	"net/http"
	"strconv"

	"github.com/gin-gonic/gin"
)

// Handler serves users.
type Handler struct{}

// User is what the API returns.
type User struct {
	ID    int    `json:"id"`
	Name  string `json:"name"`
	Email string `json:"email"`
}

// CreateUser is the body of POST /users.
type CreateUser struct {
	Name  string   `json:"name" binding:"required"`
	Email string   `json:"email" binding:"required,email"`
	Age   int      `json:"age"`
	Tags  []string `json:"tags"`
	Admin bool     `json:"-"`
}

// UpdateUser is the body of PUT /users/:id.
type UpdateUser struct {
	Name string `json:"name"`
}

func (h *Handler) List(c *gin.Context) {
	limit, _ := strconv.Atoi(c.DefaultQuery("limit", "20"))
	q := c.Query("q")
	_ = limit
	_ = q
	c.JSON(http.StatusOK, []User{})
}

func (h *Handler) Get(c *gin.Context) {
	id := c.Param("id")
	trace := c.GetHeader("X-Trace-Id")
	_ = trace
	c.JSON(http.StatusOK, User{Name: id})
}

func (h *Handler) Create(c *gin.Context) {
	var req CreateUser
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusCreated, User{Name: req.Name, Email: req.Email})
}

func (h *Handler) Update(c *gin.Context) {
	req := UpdateUser{}
	if err := c.BindJSON(&req); err != nil {
		return
	}
	c.Status(http.StatusNoContent)
}

func (h *Handler) Delete(c *gin.Context) {
	c.Status(http.StatusNoContent)
}

func (h *Handler) Avatar(c *gin.Context) {
	file, err := c.FormFile("avatar")
	if err != nil {
		c.Status(http.StatusBadRequest)
		return
	}
	_ = file
	c.Status(http.StatusNoContent)
}
