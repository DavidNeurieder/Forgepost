/* Forgepost click-to-load video embeds.
 *
 * A video block renders as a button with a lazy thumbnail and NO iframe, so
 * the reader's browser never talks to a third-party provider until they
 * actually choose to play. Clicking the button swaps in the iframe.
 *
 * Privacy baseline: every injected iframe gets referrerpolicy="no-referrer"
 * and only the `src` that the server itself built (whitelisted providers, or
 * an author-supplied https URL) is ever used as the embed target.
 */
(function () {
  'use strict';

  function iframeFrom(button) {
    var src = button.getAttribute('data-src');
    if (!src) {
      return null;
    }
    var frame = document.createElement('iframe');
    frame.className = 'video-frame';
    frame.src = src;
    frame.title = button.getAttribute('aria-label') || 'Video';
    frame.allow = 'accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share';
    frame.referrerPolicy = 'no-referrer';
    frame.allowFullscreen = true;
    return frame;
  }

  function onLoad() {
    var buttons = document.querySelectorAll('.video-box[data-video]');
    for (var i = 0; i < buttons.length; i += 1) {
      (function (button) {
        if (button.getAttribute('data-video-bound') === '1') {
          return;
        }
        button.setAttribute('data-video-bound', '1');
        button.addEventListener('click', function () {
          var frame = iframeFrom(button);
          if (!frame) {
            return;
          }
          button.replaceWith(frame);
          frame.focus();
        });
      })(buttons[i]);
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', onLoad);
  } else {
    onLoad();
  }
})();
