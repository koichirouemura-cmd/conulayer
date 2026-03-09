import os
import re
from flask import Flask, request, Response

app = Flask(__name__)
DOC_DIR = "/doc"
MAX_SIZE = 32 * 1024  # 32KB (unikernel BODY_MAX に合わせる)
VALID_NAME = re.compile(r'^[a-zA-Z0-9_\-.]+\.txt$')

def cors(resp):
    resp.headers["Access-Control-Allow-Origin"] = "*"
    return resp

@app.route("/editor/api/files", methods=["GET"])
def list_files():
    try:
        files = sorted(f for f in os.listdir(DOC_DIR) if VALID_NAME.match(f))
    except FileNotFoundError:
        files = []
    import json
    return cors(Response(json.dumps(files), content_type="application/json"))

@app.route("/editor/api/read", methods=["POST"])
def read_file():
    name = request.get_data(as_text=True).strip()
    if not VALID_NAME.match(name):
        return cors(Response("Bad Request", status=400))
    path = os.path.join(DOC_DIR, name)
    if not os.path.exists(path):
        return cors(Response("Not Found", status=404))
    with open(path, "r", encoding="utf-8") as f:
        return cors(Response(f.read(), content_type="text/plain; charset=utf-8"))

@app.route("/editor/api/save", methods=["POST"])
def save_file():
    body = request.get_data(as_text=True)
    if "\n" not in body:
        return cors(Response("Bad Request", status=400))
    name, content = body.split("\n", 1)
    name = name.strip()
    if not VALID_NAME.match(name):
        return cors(Response("Bad Request", status=400))
    if len(content.encode("utf-8")) > MAX_SIZE:
        return cors(Response("Too Large", status=413))
    os.makedirs(DOC_DIR, exist_ok=True)
    with open(os.path.join(DOC_DIR, name), "w", encoding="utf-8") as f:
        f.write(content)
    return cors(Response("OK"))

@app.route("/editor/", defaults={"path": ""})
@app.route("/editor/<path:path>")
def editor_ui(path):
    return app.send_static_file("editor_ui.html")
