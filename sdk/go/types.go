// Package kleos provides a Go client for the Kleos memory server API.
//
// The client uses only the Go standard library. Structs mirror the core domain
// objects from the OpenAPI spec served at GET /openapi.json.
package kleos

// Memory is a stored memory record.
type Memory struct {
	ID              int64    `json:"id"`
	Content         string   `json:"content"`
	Category        string   `json:"category"`
	Source          string   `json:"source,omitempty"`
	SessionID       string   `json:"session_id,omitempty"`
	Importance      int      `json:"importance"`
	Version         int      `json:"version"`
	IsLatest        bool     `json:"is_latest"`
	ParentMemoryID  *int64   `json:"parent_memory_id,omitempty"`
	RootMemoryID    *int64   `json:"root_memory_id,omitempty"`
	SourceCount     int      `json:"source_count"`
	IsStatic        bool     `json:"is_static"`
	IsForgotten     bool     `json:"is_forgotten"`
	IsArchived      bool     `json:"is_archived"`
	IsFact          bool     `json:"is_fact"`
	IsDecomposed    bool     `json:"is_decomposed"`
	ForgetAfter     string   `json:"forget_after,omitempty"`
	ForgetReason    string   `json:"forget_reason,omitempty"`
	Model           string   `json:"model,omitempty"`
	RecallHits      int      `json:"recall_hits"`
	RecallMisses    int      `json:"recall_misses"`
	AdaptiveScore   *float64 `json:"adaptive_score,omitempty"`
	PagerankScore   *float64 `json:"pagerank_score,omitempty"`
	LastAccessedAt  string   `json:"last_accessed_at,omitempty"`
	AccessCount     int      `json:"access_count"`
	Tags            string   `json:"tags,omitempty"`
	EpisodeID       *int64   `json:"episode_id,omitempty"`
	DecayScore      *float64 `json:"decay_score,omitempty"`
	Confidence      float64  `json:"confidence"`
	SyncID          string   `json:"sync_id,omitempty"`
	Status          string   `json:"status"`
	UserID          int64    `json:"user_id"`
	SpaceID         *int64   `json:"space_id,omitempty"`
	Valence         *float64 `json:"valence,omitempty"`
	Arousal         *float64 `json:"arousal,omitempty"`
	DominantEmotion string   `json:"dominant_emotion,omitempty"`
	CreatedAt       string   `json:"created_at"`
	UpdatedAt       string   `json:"updated_at"`
	IsSuperseded    bool     `json:"is_superseded"`
	IsConsolidated  bool     `json:"is_consolidated"`
}

// StoreRequest is the payload for POST /memories.
type StoreRequest struct {
	Content        string   `json:"content"`
	Category       string   `json:"category,omitempty"`
	Source         string   `json:"source,omitempty"`
	Importance     *int     `json:"importance,omitempty"`
	Tags           []string `json:"tags,omitempty"`
	SessionID      string   `json:"session_id,omitempty"`
	IsStatic       *bool    `json:"is_static,omitempty"`
	SpaceID        *int64   `json:"space_id,omitempty"`
	ParentMemoryID *int64   `json:"parent_memory_id,omitempty"`
	Agent          string   `json:"agent,omitempty"`
}

// StoreResult is the response from POST /memories.
type StoreResult struct {
	ID          int64   `json:"id"`
	Status      string  `json:"status,omitempty"`
	Created     *bool   `json:"created,omitempty"`
	DuplicateOf *int64  `json:"duplicate_of,omitempty"`
}

// UpdateRequest is the payload for POST /memory/{id}/update.
type UpdateRequest struct {
	Content    string   `json:"content,omitempty"`
	Category   string   `json:"category,omitempty"`
	Importance *int     `json:"importance,omitempty"`
	Tags       []string `json:"tags,omitempty"`
	IsStatic   *bool    `json:"is_static,omitempty"`
	IsFact     *bool    `json:"is_fact,omitempty"`
}

// SearchRequest is the payload for POST /memories/search.
type SearchRequest struct {
	Query              string   `json:"query"`
	Limit              *int     `json:"limit,omitempty"`
	Category           string   `json:"category,omitempty"`
	Source             string   `json:"source,omitempty"`
	Tags               []string `json:"tags,omitempty"`
	Threshold          *float64 `json:"threshold,omitempty"`
	SpaceID            *int64   `json:"space_id,omitempty"`
	IncludeForgotten   *bool    `json:"include_forgotten,omitempty"`
	Mode               string   `json:"mode,omitempty"`
	QuestionType       string   `json:"question_type,omitempty"`
	ExpandRelationships *bool   `json:"expand_relationships,omitempty"`
	IncludeLinks       *bool    `json:"include_links,omitempty"`
	LatestOnly         *bool    `json:"latest_only,omitempty"`
	SourceFilter       string   `json:"source_filter,omitempty"`
}

// LinkedMemory is a related memory returned in search results.
type LinkedMemory struct {
	ID         int64   `json:"id"`
	Content    string  `json:"content"`
	Category   string  `json:"category"`
	Similarity float64 `json:"similarity"`
	Type       string  `json:"type"`
}

// VersionChainEntry is one entry in a memory's version history.
type VersionChainEntry struct {
	ID       int64  `json:"id"`
	Content  string `json:"content"`
	Version  int    `json:"version"`
	IsLatest bool   `json:"is_latest"`
}

// SearchResult is a single hit from a search request.
type SearchResult struct {
	Memory                 Memory               `json:"memory"`
	Score                  float64              `json:"score"`
	SearchType             string               `json:"search_type"`
	DecayScore             *float64             `json:"decay_score,omitempty"`
	CombinedScore          *float64             `json:"combined_score,omitempty"`
	SemanticScore          *float64             `json:"semantic_score,omitempty"`
	FTSScore               *float64             `json:"fts_score,omitempty"`
	GraphScore             *float64             `json:"graph_score,omitempty"`
	PersonalitySignalScore *float64             `json:"personality_signal_score,omitempty"`
	TemporalBoost          *float64             `json:"temporal_boost,omitempty"`
	Channels               []string             `json:"channels,omitempty"`
	QuestionType           string               `json:"question_type,omitempty"`
	Reranked               *bool                `json:"reranked,omitempty"`
	RerankerMs             *float64             `json:"reranker_ms,omitempty"`
	CandidateCount         *int                 `json:"candidate_count,omitempty"`
	Linked                 []LinkedMemory       `json:"linked,omitempty"`
	VersionChain           []VersionChainEntry  `json:"version_chain,omitempty"`
}

// ContextRequest is the payload for POST /context.
type ContextRequest struct {
	Query              string   `json:"query"`
	Strategy           string   `json:"strategy,omitempty"`
	Mode               string   `json:"mode,omitempty"`
	MaxTokens          *int     `json:"max_tokens,omitempty"`
	Categories         []string `json:"categories,omitempty"`
	QuestionType       string   `json:"question_type,omitempty"`
	IncludePersonality *bool    `json:"include_personality,omitempty"`
}

// ContextBlock is one assembled context block.
type ContextBlock struct {
	Content  string   `json:"content"`
	Source   string   `json:"source"`
	MemoryID *int64   `json:"memory_id,omitempty"`
	Score    *float64 `json:"score,omitempty"`
	Category string   `json:"category,omitempty"`
}

// ContextResponse is the response from POST /context.
type ContextResponse struct {
	Blocks      []ContextBlock `json:"blocks"`
	TotalTokens int            `json:"total_tokens"`
	Strategy    string         `json:"strategy"`
	Mode        string         `json:"mode"`
}

// Agent is a registered agent record.
type Agent struct {
	ID          int64  `json:"id"`
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
	Slug        string `json:"slug,omitempty"`
	IsActive    bool   `json:"is_active"`
	CreatedAt   string `json:"created_at,omitempty"`
	UpdatedAt   string `json:"updated_at,omitempty"`
}

// CreateAgentRequest is the payload for POST /agents.
type CreateAgentRequest struct {
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
	Slug        string `json:"slug,omitempty"`
}

// AgentPassport is a signed token issued for an agent.
type AgentPassport struct {
	Token     string `json:"token"`
	AgentID   int64  `json:"agent_id"`
	IssuedAt  string `json:"issued_at,omitempty"`
	ExpiresAt string `json:"expires_at,omitempty"`
}

// GraphNode is a node in the memory graph.
type GraphNode struct {
	ID         int64              `json:"id"`
	Label      string             `json:"label"`
	NodeType   string             `json:"node_type,omitempty"`
	MemoryID   *int64             `json:"memory_id,omitempty"`
	Properties map[string]interface{} `json:"properties,omitempty"`
}

// GraphEdge is a directed edge in the memory graph.
type GraphEdge struct {
	SourceID int64    `json:"source_id"`
	TargetID int64    `json:"target_id"`
	EdgeType string   `json:"edge_type"`
	Weight   *float64 `json:"weight,omitempty"`
}

// NeighborhoodResponse is returned by GET /graph/neighborhood/{id}.
type NeighborhoodResponse struct {
	CenterID int64       `json:"center_id"`
	Nodes    []GraphNode `json:"nodes"`
	Edges    []GraphEdge `json:"edges"`
}

// SkillRef is a condensed skill reference returned in list responses.
type SkillRef struct {
	ID          int64    `json:"id"`
	Name        string   `json:"name"`
	Description string   `json:"description,omitempty"`
	Slug        string   `json:"slug,omitempty"`
	Version     string   `json:"version,omitempty"`
	Tags        []string `json:"tags,omitempty"`
	CreatedAt   string   `json:"created_at,omitempty"`
}

// SkillDetail extends SkillRef with full payload.
type SkillDetail struct {
	SkillRef
	Content        string                 `json:"content,omitempty"`
	Parameters     map[string]interface{} `json:"parameters,omitempty"`
	Dependencies   []string               `json:"dependencies,omitempty"`
	ExecutionCount *int                   `json:"execution_count,omitempty"`
	SuccessRate    *float64               `json:"success_rate,omitempty"`
}

// APIKey is an API key record.
type APIKey struct {
	ID         int64  `json:"id"`
	Name       string `json:"name,omitempty"`
	Prefix     string `json:"prefix,omitempty"`
	CreatedAt  string `json:"created_at,omitempty"`
	LastUsedAt string `json:"last_used_at,omitempty"`
	IsActive   bool   `json:"is_active"`
}

// CreateKeyRequest is the payload for POST /keys.
type CreateKeyRequest struct {
	Name    string `json:"name,omitempty"`
	SpaceID *int64 `json:"space_id,omitempty"`
}

// CreateKeyResponse includes the plaintext key (shown once).
type CreateKeyResponse struct {
	ID   int64  `json:"id"`
	Key  string `json:"key"`
	Name string `json:"name,omitempty"`
}

// BatchOperation is one operation inside a batch request.
type BatchOperation struct {
	Method string      `json:"method"`
	Path   string      `json:"path"`
	Body   interface{} `json:"body,omitempty"`
}

// BatchRequest is the payload for POST /batch.
type BatchRequest struct {
	Operations []BatchOperation `json:"operations"`
}

// BatchResultItem is the outcome of one batch operation.
type BatchResultItem struct {
	Status int         `json:"status"`
	Body   interface{} `json:"body,omitempty"`
	Error  string      `json:"error,omitempty"`
}

// BatchResponse is the response from POST /batch.
type BatchResponse struct {
	Results []BatchResultItem `json:"results"`
}

// APIError is the standard error body from the server.
type APIError struct {
	Error string `json:"error"`
	Code  string `json:"code,omitempty"`
}
