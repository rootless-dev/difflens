from flask import Blueprint, Flask
from flask.views import MethodView

app = Flask(__name__)
admin = Blueprint("admin", __name__, url_prefix="/admin")


@app.route("/")
def index() -> str:
    return "hello"


@admin.route("/users", methods=["GET", "POST"])
def users() -> str:
    return "users"


@app.before_request
def authenticate() -> None:
    return None


class ItemView(MethodView):
    def get(self, item_id: int) -> str:
        return str(item_id)

    def delete(self, item_id: int) -> str:
        return ""


app.add_url_rule("/items/<int:item_id>", view_func=ItemView.as_view("item"))
app.register_blueprint(admin)


@app.cli.command("seed")
def seed() -> None:
    print("seeding")
