from celery import Celery, shared_task

celery = Celery("app")
celery.conf.beat_schedule = {
    "nightly-cleanup": {"task": "app.tasks.cleanup", "schedule": 86400.0},
}


@celery.task(name="receipts.send")
def send_receipt(order_id: int) -> None:
    print("receipt", order_id)


@shared_task
def cleanup() -> None:
    print("cleanup")
