(function () {
    var ko = document.documentElement.lang === "ko";
    var root = new URL(path_to_root || "./", location.href);
    var rel = location.pathname.slice(root.pathname.length);
    var target = new URL(ko ? "../" + rel : "ko/" + rel, root) + location.hash;
    var bar = document.querySelector(".right-buttons");
    if (!bar) return;
    var a = document.createElement("a");
    a.href = target;
    a.className = "ics-lang";
    a.textContent = ko ? "English" : "한국어";
    a.title = ko ? "Read this page in English" : "이 페이지를 한국어로 보기";
    a.setAttribute("hreflang", ko ? "en" : "ko");
    bar.insertBefore(a, bar.firstChild);
})();
