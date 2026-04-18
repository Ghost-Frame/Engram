package kleos

import "fmt"

// KleosError is the base error type returned by the client when the server
// responds with a non-2xx status code.
type KleosError struct {
	StatusCode int
	Message    string
	Body       *APIError
}

func (e *KleosError) Error() string {
	if e.Body != nil && e.Body.Error != "" {
		return fmt.Sprintf("kleos: HTTP %d: %s", e.StatusCode, e.Body.Error)
	}
	return fmt.Sprintf("kleos: HTTP %d: %s", e.StatusCode, e.Message)
}

// IsKleosError reports whether err is an *KleosError and returns it if so.
func IsKleosError(err error) (*KleosError, bool) {
	if e, ok := err.(*KleosError); ok {
		return e, true
	}
	return nil, false
}

// IsNotFound reports whether err represents a 404 Not Found response.
func IsNotFound(err error) bool {
	if e, ok := err.(*KleosError); ok {
		return e.StatusCode == 404
	}
	return false
}

// IsUnauthorized reports whether err represents a 401 Unauthorized response.
func IsUnauthorized(err error) bool {
	if e, ok := err.(*KleosError); ok {
		return e.StatusCode == 401
	}
	return false
}

// IsForbidden reports whether err represents a 403 Forbidden response.
func IsForbidden(err error) bool {
	if e, ok := err.(*KleosError); ok {
		return e.StatusCode == 403
	}
	return false
}

// IsValidationError reports whether err represents a 400 or 422 response.
func IsValidationError(err error) bool {
	if e, ok := err.(*KleosError); ok {
		return e.StatusCode == 400 || e.StatusCode == 422
	}
	return false
}

// IsRateLimited reports whether err represents a 429 Too Many Requests response.
func IsRateLimited(err error) bool {
	if e, ok := err.(*KleosError); ok {
		return e.StatusCode == 429
	}
	return false
}

// IsServerError reports whether err represents a 5xx server error.
func IsServerError(err error) bool {
	if e, ok := err.(*KleosError); ok {
		return e.StatusCode >= 500
	}
	return false
}
