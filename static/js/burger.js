document.addEventListener('DOMContentLoaded', () => {
    // Get all "navbar-burger" elements and add event listeners
    Array.prototype.slice.call(document.querySelectorAll('.navbar-burger'), 0)
        .forEach(el => {
            el.addEventListener('click', () => {
                // Get the target from the "data-target" attribute
                const $target = document.getElementById(el.dataset.target);
                // Toggle the "is-active" class on both the "navbar-burger" and the "navbar-menu"
                el.classList.toggle('is-active');
                $target.classList.toggle('is-active');
            });
        });
});
