from fastapi import APIRouter, BackgroundTasks, FastAPI

from app import repository
from app.tasks import send_receipt

app = FastAPI()
router = APIRouter(prefix="/orders")


@router.get("/{order_id}")
async def get_order(order_id: int) -> dict:
    return repository.find(order_id)


@router.post("")
async def create_order(payload: dict, background: BackgroundTasks) -> dict:
    order = repository.save(payload)
    background.add_task(notify, order["id"])
    send_receipt.delay(order["id"])
    return order


@app.middleware("http")
async def timing(request, call_next):
    return await call_next(request)


def notify(order_id: int) -> None:
    print(order_id)


def create_app() -> FastAPI:
    api = FastAPI()

    @api.get("/health")
    def health() -> dict:
        return {"ok": True}

    api.add_api_route("/version", version, methods=["GET"])
    api.include_router(router)
    return api


def version() -> str:
    return "1"


app.include_router(router)
