import re
content = """
async fn admin_gc(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<GcBody>>,
) -> Result<Json<Value>, AppError> {
"""
m = re.search(r"async fn ([a-zA-Z0-9_]+)\s*\(([^)]*)\)", content)
print(m.group(2) if m else "no match")
print("Auth" in m.group(2) if m else "")
