import { Button as ButtonPrimitive } from "@base-ui/react/button"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "cn"

const buttonVariants = cva(
  "group/button inline-flex shrink-0 cursor-pointer items-center justify-center rounded-[7px] border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap transition-colors outline-none select-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-45 aria-invalid:border-destructive [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground not-disabled:hover:brightness-110",
        secondary:
          "border-border bg-secondary font-normal text-secondary-foreground not-disabled:hover:border-faint",
        outline:
          "rounded-sm border-border font-normal text-muted-foreground not-disabled:hover:border-faint not-disabled:hover:text-foreground",
        ghost:
          "rounded-sm text-foreground not-disabled:hover:bg-secondary",
        destructive:
          "border-border bg-secondary font-normal text-destructive not-disabled:hover:border-destructive",
        link: "h-auto rounded-sm border-0 p-0 font-normal text-muted-foreground not-disabled:hover:text-foreground",
      },
      size: {
        default: "h-[35px] gap-1.5 px-3.5",
        sm: "h-[29px] gap-1 px-2.5",
        xs: "h-[27px] gap-1 px-2",
        inline: "",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  ...props
}: ButtonPrimitive.Props & VariantProps<typeof buttonVariants>) {
  return (
    <ButtonPrimitive
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
