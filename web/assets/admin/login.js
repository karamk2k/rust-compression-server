(function () {
    var params = new URLSearchParams(window.location.search);
    if (params.get("error") === "invalid_credentials") {
        var error = document.getElementById("login-error");
        if (error) {
            error.hidden = false;
        }
    }
})();
