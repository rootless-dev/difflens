from django.urls import path

from . import views

urlpatterns = [
    path("", views.home, name="home"),
    path("profiles/<int:pk>/", views.ProfileView.as_view(), name="profile"),
]
