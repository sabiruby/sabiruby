# SabiRuby's own Ruby part (the reference has none of it): the rest of a block-taking native,
# for when a SEND called it. The native does what comes before its block itself, and then
# hands the loop that calls the block to one of these, in a frame of its own
# (`Vm::send_in_frame`), so the block runs in an ordinary frame and a task can wait inside it
# (`docs/design/wait-anywhere.md`). Called from anywhere else (`funcall`) the native keeps its
# own loop. Each one does what the native's loop does, step for step: the same order of calls,
# the same arguments to the block, the array read again where the native reads it again.
#
# Compiled by the reference `mrbc` into block-frames.mrb (`tools/fixtures.sh`), without debug
# information, so none of these shows up in a backtrace, as the native does not.

class Array
  # `index { }` (array.rs `index`)
  def __index_by
    i = 0
    while i < size
      return i if yield(self[i])
      i += 1
    end
    nil
  end

  # `rindex { }` (array.rs `rindex`): the length is read again at every step, since the block
  # may shrink the array
  def __rindex_by
    i = size
    while i > 0
      i -= 1
      len = size
      if i >= len
        i = len
        next
      end
      return i if yield(self[i])
    end
    nil
  end

  # `Array.new(n) { |i| }` (array.rs `initialize`): the elements are collected first and the
  # receiver is filled once, as the native fills it
  def __init_by(n)
    a = []
    i = 0
    while i < n
      a << yield(i)
      i += 1
    end
    replace(a)
  end

  # `sort! { |a, b| }` (array.rs `sort_values`): the same bottom-up merge sort, comparing the
  # same pairs in the same order, so an inconsistent block gives the same order it gave there.
  # `__sort_cmp` reads the block's answer as the native reads it (an Integer, nil is an error,
  # anything else is asked `> 0` and `< 0`).
  def __sort_by_block!
    src = [].replace(self)
    n = src.size
    dst = [].replace(src)
    width = 1
    while width < n
      i = 0
      while i < n
        mid = i + width
        mid = n if mid > n
        hi = i + 2 * width
        hi = n if hi > n
        l = i
        r = mid
        k = i
        while l < mid && r < hi
          x = src[r]
          y = src[l]
          if __sort_cmp(yield(x, y), x, y) < 0
            dst[k] = x
            r += 1
          else
            dst[k] = y
            l += 1
          end
          k += 1
        end
        while l < mid
          dst[k] = src[l]
          l += 1
          k += 1
        end
        while r < hi
          dst[k] = src[r]
          r += 1
          k += 1
        end
        i += 2 * width
      end
      t = src
      src = dst
      dst = t
      width *= 2
    end
    replace(src)
  end

  private :__index_by, :__rindex_by, :__init_by, :__sort_by_block!
end
