from django.dispatch import receiver
from django.db.models.signals import post_save
from django.http import HttpResponse
from django.views import View


def home(request):
    return HttpResponse("home")


class ProfileView(View):
    def get(self, request, pk):
        return HttpResponse(str(pk))

    def post(self, request, pk):
        return HttpResponse("saved")


@receiver(post_save)
def audit(sender, **kwargs):
    print(sender)
